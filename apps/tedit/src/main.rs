//! tedit – Texteditor von TUILNUX.

mod buffer;

use std::io::{self, Write};
use std::path::PathBuf;

use base64::Engine;
use buffer::{Buffer, Pos, col_from_display, display_col};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::{DefaultTerminal, Frame};
use tuilnux_ui::{App, Flow, InputResult, LineInput, keybar, status_line, theme};

const MAX_FILE_BYTES: u64 = 64 * 1024 * 1024;
const UNDO_LIMIT: usize = 500;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EditKind {
    None,
    Typing,
    Deleting,
    Other,
}

#[derive(Debug, Clone, PartialEq)]
enum InputKind {
    Find,
    ReplaceFind,
    ReplaceWith(String),
    Goto,
    SaveAs,
}

impl InputKind {
    fn label(&self) -> String {
        match self {
            InputKind::Find => " Suchen: ".into(),
            InputKind::ReplaceFind => " Ersetzen – suchen nach: ".into(),
            InputKind::ReplaceWith(f) => format!(" „{f}“ ersetzen durch: "),
            InputKind::Goto => " Gehe zu Zeile: ".into(),
            InputKind::SaveAs => " Speichern unter: ".into(),
        }
    }
}

struct Snapshot {
    lines: Vec<String>,
    cursor: Pos,
}

struct Editor {
    path: Option<PathBuf>,
    buf: Buffer,
    crlf: bool,
    cursor: Pos,
    anchor: Option<Pos>,
    want_x: Option<usize>,
    scroll_row: usize,
    scroll_x: usize,
    modified: bool,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    last_edit: EditKind,
    clipboard: String,
    input: Option<(InputKind, LineInput)>,
    search: String,
    message: Option<(String, bool)>,
    quit_armed: bool,
    text_area: Rect,
    gutter: u16,
    dragging: bool,
}

/// Kopiert Text über OSC 52 auch in die Zwischenablage des Terminals.
fn osc52_copy(text: &str) {
    let data = base64::engine::general_purpose::STANDARD.encode(text);
    let mut out = io::stdout();
    let _ = write!(out, "\x1b]52;c;{data}\x07");
    let _ = out.flush();
}

fn expand_home(text: &str) -> PathBuf {
    match text.strip_prefix("~/") {
        Some(rest) => std::env::home_dir().map_or_else(|| PathBuf::from(text), |h| h.join(rest)),
        None => PathBuf::from(text),
    }
}

impl Editor {
    fn open(path: Option<PathBuf>) -> anyhow::Result<Self> {
        let mut message = None;
        let (buf, crlf) = match &path {
            Some(p) if p.exists() => {
                let size = std::fs::metadata(p)?.len();
                anyhow::ensure!(
                    size <= MAX_FILE_BYTES,
                    "{}: Datei ist größer als 64 MiB",
                    p.display()
                );
                let text = String::from_utf8(std::fs::read(p)?)
                    .map_err(|_| anyhow::anyhow!("{}: keine UTF-8-Textdatei", p.display()))?;
                Buffer::from_text(&text)
            }
            Some(_) => {
                message = Some(("Neue Datei – Ctrl+S speichert".to_string(), false));
                Buffer::from_text("")
            }
            None => Buffer::from_text(""),
        };
        Ok(Self {
            path,
            buf,
            crlf,
            cursor: Pos::default(),
            anchor: None,
            want_x: None,
            scroll_row: 0,
            scroll_x: 0,
            modified: false,
            undo: Vec::new(),
            redo: Vec::new(),
            last_edit: EditKind::None,
            clipboard: String::new(),
            input: None,
            search: String::new(),
            message,
            quit_armed: false,
            text_area: Rect::default(),
            gutter: 5,
            dragging: false,
        })
    }

    fn info(&mut self, msg: impl Into<String>) {
        self.message = Some((msg.into(), false));
    }

    fn error(&mut self, msg: impl Into<String>) {
        self.message = Some((msg.into(), true));
    }

    fn file_name(&self) -> String {
        self.path
            .as_ref()
            .and_then(|p| p.file_name())
            .map_or_else(|| "[Neu]".to_string(), |n| n.to_string_lossy().into_owned())
    }

    // Auswahl

    fn selection(&self) -> Option<(Pos, Pos)> {
        self.anchor
            .filter(|a| *a != self.cursor)
            .map(|a| (a.min(self.cursor), a.max(self.cursor)))
    }

    fn move_to(&mut self, pos: Pos, select: bool) {
        if select {
            self.anchor.get_or_insert(self.cursor);
        } else {
            self.anchor = None;
        }
        self.cursor = self.buf.clamp(pos);
        self.last_edit = EditKind::None;
        self.want_x = None;
    }

    // Bearbeiten mit Rückgängig

    fn checkpoint(&mut self, kind: EditKind) {
        if kind == EditKind::Other || kind != self.last_edit {
            self.undo.push(Snapshot {
                lines: self.buf.lines.clone(),
                cursor: self.cursor,
            });
            if self.undo.len() > UNDO_LIMIT {
                self.undo.remove(0);
            }
            self.redo.clear();
        }
        self.last_edit = kind;
        self.modified = true;
        self.want_x = None;
    }

    fn restore(&mut self, redo: bool) {
        let (from, to) = if redo {
            (&mut self.redo, &mut self.undo)
        } else {
            (&mut self.undo, &mut self.redo)
        };
        let Some(snap) = from.pop() else {
            self.info(if redo {
                "Nichts zum Wiederholen"
            } else {
                "Nichts rückgängig zu machen"
            });
            return;
        };
        to.push(Snapshot {
            lines: std::mem::replace(&mut self.buf.lines, snap.lines),
            cursor: self.cursor,
        });
        self.cursor = self.buf.clamp(snap.cursor);
        self.anchor = None;
        self.modified = true;
        self.last_edit = EditKind::None;
    }

    fn delete_selection(&mut self) {
        if let Some((a, b)) = self.selection() {
            self.buf.delete(a, b);
            self.cursor = a;
        }
        self.anchor = None;
    }

    fn insert_text(&mut self, text: &str, kind: EditKind) {
        let boundary = kind == EditKind::Typing && text.chars().any(char::is_whitespace);
        let kind = if self.selection().is_some() || boundary {
            EditKind::Other
        } else {
            kind
        };
        self.checkpoint(kind);
        self.delete_selection();
        self.cursor = self.buf.insert(self.cursor, text);
        if boundary {
            // Nach einem Leerzeichen beginnt eine neue Rückgängig-Gruppe.
            self.last_edit = EditKind::Other;
        }
    }

    fn newline(&mut self) {
        let indent: String = self.buf.lines[self.cursor.row]
            .chars()
            .take(self.cursor.col)
            .take_while(|c| *c == ' ' || *c == '\t')
            .collect();
        self.insert_text(&format!("\n{indent}"), EditKind::Other);
    }

    fn backspace(&mut self, word: bool) {
        if self.selection().is_some() {
            self.checkpoint(EditKind::Other);
            self.delete_selection();
            return;
        }
        let start = if word {
            self.buf.word_left(self.cursor)
        } else {
            self.buf.prev(self.cursor)
        };
        if start != self.cursor {
            self.checkpoint(EditKind::Deleting);
            self.buf.delete(start, self.cursor);
            self.cursor = start;
        }
    }

    fn delete_forward(&mut self, word: bool) {
        if self.selection().is_some() {
            self.checkpoint(EditKind::Other);
            self.delete_selection();
            return;
        }
        let end = if word {
            self.buf.word_right(self.cursor)
        } else {
            self.buf.next(self.cursor)
        };
        if end != self.cursor {
            self.checkpoint(EditKind::Deleting);
            self.buf.delete(self.cursor, end);
        }
    }

    /// Bereich einer ganzen Zeile inklusive Zeilenumbruch.
    fn line_range(&self, row: usize) -> (Pos, Pos) {
        if row + 1 < self.buf.lines.len() {
            (Pos::new(row, 0), Pos::new(row + 1, 0))
        } else if row > 0 {
            (
                Pos::new(row - 1, self.buf.line_len(row - 1)),
                Pos::new(row, self.buf.line_len(row)),
            )
        } else {
            (Pos::new(0, 0), Pos::new(0, self.buf.line_len(0)))
        }
    }

    fn delete_line(&mut self) {
        let row = self.cursor.row;
        let (a, b) = self.line_range(row);
        self.checkpoint(EditKind::Other);
        self.buf.delete(a, b);
        self.cursor = self.buf.clamp(Pos::new(row, 0));
        self.anchor = None;
    }

    fn copy(&mut self, cut: bool) {
        let text = match self.selection() {
            Some((a, b)) => self.buf.text(a, b),
            None => format!("{}\n", self.buf.lines[self.cursor.row]),
        };
        osc52_copy(&text);
        self.clipboard = text;
        if !cut {
            self.info("Kopiert");
        } else if self.selection().is_some() {
            self.checkpoint(EditKind::Other);
            self.delete_selection();
            self.info("Ausgeschnitten");
        } else {
            self.delete_line();
            self.info("Zeile ausgeschnitten");
        }
    }

    fn indent(&mut self, outdent: bool) {
        let (first, last) = match self.selection() {
            Some((a, b)) => (
                a.row,
                if b.col == 0 && b.row > a.row {
                    b.row - 1
                } else {
                    b.row
                },
            ),
            None if !outdent => {
                let x = display_col(&self.buf.lines[self.cursor.row], self.cursor.col);
                let pad = buffer::TAB_WIDTH - x % buffer::TAB_WIDTH;
                self.insert_text(&" ".repeat(pad), EditKind::Typing);
                return;
            }
            None => (self.cursor.row, self.cursor.row),
        };
        self.checkpoint(EditKind::Other);
        for row in first..=last {
            let line = &mut self.buf.lines[row];
            let shift = if outdent {
                let n = line
                    .chars()
                    .take(buffer::TAB_WIDTH)
                    .take_while(|c| *c == ' ')
                    .count();
                line.drain(..n);
                -(n as isize)
            } else {
                line.insert_str(0, "    ");
                buffer::TAB_WIDTH as isize
            };
            for p in [Some(&mut self.cursor), self.anchor.as_mut()]
                .into_iter()
                .flatten()
            {
                if p.row == row {
                    p.col = (p.col as isize + shift).max(0) as usize;
                }
            }
        }
    }

    // Datei, Suche, Navigation

    fn save(&mut self) {
        let Some(path) = self.path.clone() else {
            self.input = Some((InputKind::SaveAs, LineInput::default()));
            return;
        };
        let text = self.buf.to_text(self.crlf);
        match std::fs::write(&path, &text) {
            Ok(()) => {
                self.modified = false;
                self.quit_armed = false;
                self.info(format!(
                    "Gespeichert: {} ({} Zeilen, {})",
                    path.display(),
                    self.buf.lines.len(),
                    tuilnux_ui::human_size(text.len() as u64)
                ));
            }
            Err(e) => self.error(format!("Speichern fehlgeschlagen: {e}")),
        }
    }

    fn find_next(&mut self, forward: bool) {
        if self.search.is_empty() {
            self.input = Some((InputKind::Find, LineInput::default()));
            return;
        }
        let from = if forward {
            self.buf
                .next(self.selection().map_or(self.cursor, |(a, _)| a))
        } else {
            self.selection().map_or(self.cursor, |(a, _)| a)
        };
        match self.buf.find(&self.search, from, forward) {
            Some(pos) => {
                self.anchor = Some(pos);
                self.cursor = Pos::new(pos.row, pos.col + self.search.chars().count());
                self.info(format!(
                    "„{}“ in Zeile {} (F3 weiter, ⇧F3 zurück)",
                    self.search,
                    pos.row + 1
                ));
            }
            None => self.error(format!("„{}“ nicht gefunden", self.search)),
        }
    }

    fn submit_input(&mut self, kind: InputKind, text: String) {
        match kind {
            InputKind::Find => {
                self.search = text;
                if let Some((a, _)) = self.selection() {
                    self.cursor = a;
                }
                self.anchor = None;
                // Suche beginnt an der aktuellen Position (inklusive).
                let pos = self.cursor;
                match self.buf.find(&self.search, pos, true) {
                    Some(p) => {
                        self.anchor = Some(p);
                        self.cursor = Pos::new(p.row, p.col + self.search.chars().count());
                        self.info(format!(
                            "„{}“ in Zeile {} (F3 weiter, ⇧F3 zurück)",
                            self.search,
                            p.row + 1
                        ));
                    }
                    None if self.search.is_empty() => {}
                    None => self.error(format!("„{}“ nicht gefunden", self.search)),
                }
            }
            InputKind::ReplaceFind if !text.is_empty() => {
                self.input = Some((InputKind::ReplaceWith(text), LineInput::default()));
            }
            InputKind::ReplaceFind => {}
            InputKind::ReplaceWith(find) => {
                let mut changed = self.buf.clone();
                let n = changed.replace_all(&find, &text);
                if n == 0 {
                    self.error(format!(
                        "„{find}“ nicht gefunden (Groß-/Kleinschreibung beachten)"
                    ));
                } else {
                    self.checkpoint(EditKind::Other);
                    self.buf = changed;
                    self.cursor = self.buf.clamp(self.cursor);
                    self.anchor = None;
                    self.info(format!("{n}× ersetzt (Ctrl+Z macht es rückgängig)"));
                }
            }
            InputKind::Goto => match text.trim().parse::<usize>() {
                Ok(n) if n >= 1 => self.move_to(Pos::new(n - 1, 0), false),
                _ => self.error("Bitte eine Zeilennummer eingeben"),
            },
            InputKind::SaveAs => {
                let text = text.trim();
                if !text.is_empty() {
                    self.path = Some(expand_home(text));
                    self.save();
                }
            }
        }
    }

    fn vertical(&mut self, rows: isize, select: bool) {
        let x = self
            .want_x
            .unwrap_or_else(|| display_col(&self.buf.lines[self.cursor.row], self.cursor.col));
        let row =
            (self.cursor.row as isize + rows).clamp(0, self.buf.lines.len() as isize - 1) as usize;
        let col = col_from_display(&self.buf.lines[row], x);
        self.move_to(Pos::new(row, col), select);
        self.want_x = Some(x);
    }

    fn pos_at(&self, column: u16, row: u16) -> Pos {
        let area = self.text_area;
        let y = row.clamp(area.y, area.bottom().saturating_sub(1)) - area.y;
        let line_row = (self.scroll_row + y as usize).min(self.buf.lines.len() - 1);
        let x = column.saturating_sub(area.x + self.gutter) as usize + self.scroll_x;
        Pos::new(line_row, col_from_display(&self.buf.lines[line_row], x))
    }

    fn scroll_into_view(&mut self) {
        let height = self.text_area.height.max(1) as usize;
        let width = self.text_area.width.saturating_sub(self.gutter).max(1) as usize;
        if self.cursor.row < self.scroll_row {
            self.scroll_row = self.cursor.row;
        } else if self.cursor.row >= self.scroll_row + height {
            self.scroll_row = self.cursor.row + 1 - height;
        }
        let x = display_col(&self.buf.lines[self.cursor.row], self.cursor.col);
        if x < self.scroll_x {
            self.scroll_x = x.saturating_sub(8);
        } else if x >= self.scroll_x + width {
            self.scroll_x = x + 1 - width + 8.min(width / 2);
        }
    }

    fn render_line(&self, row: usize, width: usize) -> Line<'static> {
        let sel = self.selection();
        let normal = Style::new().fg(theme::fg());
        let selected = Style::new().fg(Color::White).bg(theme::sel_bg());
        let line = &self.buf.lines[row];
        let len = line.chars().count();
        // Am Zeilenende eine Zelle markieren, wenn der Zeilenumbruch mit ausgewählt ist.
        let newline_selected = sel.is_some_and(|(a, b)| a.row <= row && b.row > row);
        let in_sel = |col: usize| {
            sel.is_some_and(|(a, b)| Pos::new(row, col) >= a && Pos::new(row, col) < b)
        };

        let mut spans: Vec<Span<'static>> = Vec::new();
        let mut run = String::new();
        let mut run_style = normal;
        let mut x = 0usize;
        for (col, c) in line
            .chars()
            .chain(newline_selected.then_some(' '))
            .enumerate()
        {
            let w = buffer::char_width(c, x);
            if x + w <= self.scroll_x {
                x += w;
                continue;
            }
            if x >= self.scroll_x + width {
                break;
            }
            let style = if col >= len || in_sel(col) {
                selected
            } else {
                normal
            };
            if style != run_style && !run.is_empty() {
                spans.push(Span::styled(std::mem::take(&mut run), run_style));
            }
            run_style = style;
            if c == '\t' || x < self.scroll_x {
                run.push_str(&" ".repeat(x + w - x.max(self.scroll_x)));
            } else if c.is_control() {
                run.push('·');
            } else {
                run.push(c);
            }
            x += w;
        }
        if !run.is_empty() {
            spans.push(Span::styled(run, run_style));
        }
        Line::from(spans)
    }

    fn save_as_prompt(&mut self) {
        let current = self
            .path
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        self.input = Some((InputKind::SaveAs, LineInput::new(current)));
    }
}

impl App for Editor {
    fn draw(&mut self, frame: &mut Frame) {
        let [top, text, status, keys] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .areas(frame.area());

        self.gutter = (self.buf.lines.len().to_string().len().max(3) + 3) as u16;
        self.text_area = text;
        self.scroll_into_view();

        let title = format!(
            " {}{} ",
            self.path
                .as_ref()
                .map_or_else(|| "[Neu]".into(), |p| p.display().to_string()),
            if self.modified { " [+]" } else { "" }
        );
        frame.render_widget(
            Line::from(vec![
                Span::styled(" tedit ", Style::new().fg(Color::Black).bg(theme::accent())),
                Span::styled(
                    title,
                    Style::new().fg(theme::fg()).add_modifier(Modifier::BOLD),
                ),
            ]),
            top,
        );

        let width = text.width.saturating_sub(self.gutter) as usize;
        let num_width = self.gutter as usize - 3;
        let lines: Vec<Line> = (self.scroll_row..self.buf.lines.len())
            .take(text.height as usize)
            .map(|row| {
                let color = if row == self.cursor.row {
                    theme::accent()
                } else {
                    theme::dim()
                };
                let mut line = self.render_line(row, width);
                line.spans.insert(
                    0,
                    Span::styled(
                        format!("{:>num_width$} │ ", row + 1),
                        Style::new().fg(color),
                    ),
                );
                line
            })
            .collect();
        frame.render_widget(Paragraph::new(lines), text);

        if let Some((kind, input)) = &self.input {
            input.render(frame, status, &kind.label());
        } else {
            let right = format!(
                "Ln {}, Sp {}  {}  UTF-8 ",
                self.cursor.row + 1,
                display_col(&self.buf.lines[self.cursor.row], self.cursor.col) + 1,
                if self.crlf { "CRLF" } else { "LF" },
            );
            let line = match &self.message {
                Some((msg, err)) => {
                    let mut l = status_line(format!(" {msg}"), right, status.width);
                    if *err {
                        l.spans[0].style = Style::new().fg(theme::error());
                    }
                    l
                }
                None => status_line(
                    format!(
                        " {} · {} Zeilen · F1 Hilfe",
                        self.file_name(),
                        self.buf.lines.len()
                    ),
                    right,
                    status.width,
                ),
            };
            frame.render_widget(line, status);
            let x = display_col(&self.buf.lines[self.cursor.row], self.cursor.col)
                .saturating_sub(self.scroll_x);
            frame.set_cursor_position((
                text.x + self.gutter + x as u16,
                text.y + (self.cursor.row - self.scroll_row) as u16,
            ));
        }

        frame.render_widget(
            keybar(&[
                ("^S", "Speichern"),
                ("^F", "Suchen"),
                ("^H", "Ersetzen"),
                ("^G", "Zeile"),
                ("^Z", "Rückgängig"),
                ("^C", "Kopieren"),
                ("^V", "Einfügen"),
                ("^Q", "Beenden"),
            ]),
            keys,
        );
    }

    fn on_key(&mut self, key: KeyEvent, _terminal: &mut DefaultTerminal) -> io::Result<Flow> {
        if let Some((kind, mut input)) = self.input.take() {
            match input.handle_key(key) {
                InputResult::Submit => self.submit_input(kind, input.text().to_string()),
                InputResult::Cancel => self.message = None,
                InputResult::Continue => self.input = Some((kind, input)),
            }
            return Ok(Flow::Continue);
        }

        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let page = self.text_area.height.saturating_sub(1).max(1) as isize;
        if !(ctrl && key.code == KeyCode::Char('q')) {
            self.quit_armed = false;
        }
        self.message = None;

        match key.code {
            KeyCode::Char('q') if ctrl => {
                if self.modified && !self.quit_armed {
                    self.quit_armed = true;
                    self.error("Ungespeicherte Änderungen! Ctrl+Q nochmal = verwerfen, Ctrl+S = speichern");
                } else {
                    return Ok(Flow::Quit);
                }
            }
            KeyCode::Char('s' | 'S') if ctrl && (shift || alt) => self.save_as_prompt(),
            KeyCode::F(12) => self.save_as_prompt(),
            KeyCode::Char('s') if ctrl => self.save(),
            KeyCode::Char('f') if ctrl => {
                let prefill = self
                    .selection()
                    .filter(|(a, b)| a.row == b.row)
                    .map_or_else(|| self.search.clone(), |(a, b)| self.buf.text(a, b));
                self.input = Some((InputKind::Find, LineInput::new(prefill)));
            }
            KeyCode::Char('h' | 'r') if ctrl => {
                self.input = Some((InputKind::ReplaceFind, LineInput::new(self.search.clone())))
            }
            KeyCode::Char('g') if ctrl => self.input = Some((InputKind::Goto, LineInput::default())),
            KeyCode::F(3) => self.find_next(!shift),
            KeyCode::Char('n') if ctrl => self.find_next(true),
            KeyCode::Char('z') if ctrl => self.restore(false),
            KeyCode::Char('y') if ctrl => self.restore(true),
            KeyCode::Char('a') if ctrl => {
                self.anchor = Some(Pos::default());
                self.cursor = self.buf.end();
            }
            KeyCode::Char('c') if ctrl => self.copy(false),
            KeyCode::Char('x') if ctrl => self.copy(true),
            KeyCode::Char('v') if ctrl => {
                if self.clipboard.is_empty() {
                    self.info("Zwischenablage leer – Text aus dem System mit Ctrl+Shift+V einfügen");
                } else {
                    let text = self.clipboard.clone();
                    self.insert_text(&text, EditKind::Other);
                }
            }
            KeyCode::Char('d') if ctrl => {
                let line = self.buf.lines[self.cursor.row].clone();
                self.checkpoint(EditKind::Other);
                self.buf.lines.insert(self.cursor.row + 1, line);
                self.cursor.row += 1;
                self.anchor = None;
            }
            KeyCode::Char('k') if ctrl => self.delete_line(),
            KeyCode::Char(c) if !ctrl || alt => self.insert_text(&c.to_string(), EditKind::Typing),
            KeyCode::Enter => self.newline(),
            KeyCode::Tab => self.indent(false),
            KeyCode::BackTab => self.indent(true),
            KeyCode::Backspace => self.backspace(ctrl),
            KeyCode::Delete => self.delete_forward(ctrl),
            KeyCode::Left if ctrl => self.move_to(self.buf.word_left(self.cursor), shift),
            KeyCode::Right if ctrl => self.move_to(self.buf.word_right(self.cursor), shift),
            KeyCode::Left => {
                let target = match self.selection() {
                    Some((a, _)) if !shift => a,
                    _ => self.buf.prev(self.cursor),
                };
                self.move_to(target, shift);
            }
            KeyCode::Right => {
                let target = match self.selection() {
                    Some((_, b)) if !shift => b,
                    _ => self.buf.next(self.cursor),
                };
                self.move_to(target, shift);
            }
            KeyCode::Up => self.vertical(-1, shift),
            KeyCode::Down => self.vertical(1, shift),
            KeyCode::PageUp => self.vertical(-page, shift),
            KeyCode::PageDown => self.vertical(page, shift),
            KeyCode::Home if ctrl => self.move_to(Pos::default(), shift),
            KeyCode::End if ctrl => self.move_to(self.buf.end(), shift),
            KeyCode::Home => {
                let indent = self.buf.lines[self.cursor.row].chars().take_while(|c| c.is_whitespace()).count();
                let col = if self.cursor.col == indent { 0 } else { indent };
                self.move_to(Pos::new(self.cursor.row, col), shift);
            }
            KeyCode::End => {
                let end = Pos::new(self.cursor.row, self.buf.line_len(self.cursor.row));
                self.move_to(end, shift);
            }
            KeyCode::Esc => self.anchor = None,
            KeyCode::F(1) => self.info(
                "^S speichern · F12 speichern unter · ^F/F3 suchen · ^H ersetzen · ^G Zeile · ^D duplizieren · ^K Zeile löschen · ^A alles · Tab/⇧Tab einrücken",
            ),
            _ => {}
        }
        Ok(Flow::Continue)
    }

    fn on_mouse(&mut self, mouse: MouseEvent) -> Flow {
        if self.input.is_some() {
            return Flow::Continue;
        }
        let area = self.text_area;
        let inside = mouse.row >= area.y && mouse.row < area.bottom();
        let height = area.height.max(1) as usize;
        match mouse.kind {
            MouseEventKind::ScrollUp => {
                self.scroll_row = self.scroll_row.saturating_sub(3);
                if self.cursor.row >= self.scroll_row + height {
                    self.vertical(
                        (self.scroll_row + height - 1) as isize - self.cursor.row as isize,
                        false,
                    );
                }
            }
            MouseEventKind::ScrollDown => {
                self.scroll_row = (self.scroll_row + 3).min(self.buf.lines.len().saturating_sub(1));
                if self.cursor.row < self.scroll_row {
                    self.vertical(self.scroll_row as isize - self.cursor.row as isize, false);
                }
            }
            MouseEventKind::Down(MouseButton::Left) if inside => {
                let pos = self.pos_at(mouse.column, mouse.row);
                self.move_to(pos, false);
                self.anchor = Some(pos);
                self.dragging = true;
            }
            MouseEventKind::Drag(MouseButton::Left) if self.dragging => {
                self.cursor = self.pos_at(mouse.column, mouse.row);
            }
            MouseEventKind::Up(MouseButton::Left) => self.dragging = false,
            _ => {}
        }
        Flow::Continue
    }

    fn on_paste(&mut self, text: String) {
        self.insert_text(&text, EditKind::Other);
    }
}

fn main() -> anyhow::Result<()> {
    let path = match std::env::args().nth(1) {
        Some(a) if a == "-h" || a == "--help" => {
            println!("tedit [datei] – Texteditor von TUILNUX (F1 zeigt Tastenkürzel)");
            return Ok(());
        }
        Some(a) => Some(expand_home(&a)),
        None => None,
    };
    let mut editor = Editor::open(path)?;
    tuilnux_ui::run(&mut editor)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::{CrosstermBackend, TestBackend};
    use ratatui::{Terminal, TerminalOptions, Viewport};

    fn dummy_terminal() -> DefaultTerminal {
        Terminal::with_options(
            CrosstermBackend::new(io::stdout()),
            TerminalOptions {
                viewport: Viewport::Fixed(Rect::new(0, 0, 80, 24)),
            },
        )
        .unwrap()
    }

    fn press(ed: &mut Editor, term: &mut DefaultTerminal, code: KeyCode, mods: KeyModifiers) {
        ed.on_key(KeyEvent::new(code, mods), term).unwrap();
    }

    fn render(ed: &mut Editor) -> String {
        let mut t = Terminal::new(TestBackend::new(60, 12)).unwrap();
        t.draw(|f| ed.draw(f)).unwrap();
        let buf = t.backend().buffer().clone();
        (0..buf.area.height)
            .map(|y| {
                (0..buf.area.width)
                    .map(|x| buf[(x, y)].symbol().to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn typing_undo_and_save() {
        let dir = std::env::temp_dir().join(format!("tedit-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("a.txt");
        std::fs::write(&path, "hallo\r\nwelt\r\n").unwrap();
        let mut ed = Editor::open(Some(path.clone())).unwrap();
        let mut term = dummy_terminal();
        assert!(ed.crlf);
        press(&mut ed, &mut term, KeyCode::End, KeyModifiers::NONE);
        for c in " du".chars() {
            press(&mut ed, &mut term, KeyCode::Char(c), KeyModifiers::NONE);
        }
        press(&mut ed, &mut term, KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(ed.buf.lines[0], "hallo du");
        assert_eq!(ed.cursor, Pos::new(1, 0));
        press(
            &mut ed,
            &mut term,
            KeyCode::Char('z'),
            KeyModifiers::CONTROL,
        );
        assert_eq!(ed.buf.lines[0], "hallo du");
        assert_eq!(ed.buf.lines.len(), 3);
        press(
            &mut ed,
            &mut term,
            KeyCode::Char('z'),
            KeyModifiers::CONTROL,
        );
        press(
            &mut ed,
            &mut term,
            KeyCode::Char('z'),
            KeyModifiers::CONTROL,
        );
        assert_eq!(ed.buf.lines[0], "hallo");
        press(
            &mut ed,
            &mut term,
            KeyCode::Char('y'),
            KeyModifiers::CONTROL,
        );
        assert_eq!(ed.buf.lines[0], "hallo ");
        press(
            &mut ed,
            &mut term,
            KeyCode::Char('y'),
            KeyModifiers::CONTROL,
        );
        assert_eq!(ed.buf.lines[0], "hallo du");
        press(
            &mut ed,
            &mut term,
            KeyCode::Char('s'),
            KeyModifiers::CONTROL,
        );
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "hallo du\r\nwelt\r\n"
        );
        assert!(!ed.modified);
        let screen = render(&mut ed);
        assert!(screen.contains("hallo du"), "{screen}");
        assert!(screen.contains("CRLF"), "{screen}");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn find_replace_and_quit_guard() {
        let mut ed = Editor::open(None).unwrap();
        let mut term = dummy_terminal();
        ed.on_paste("eins zwei\nzwei drei\n".into());
        ed.search = "zwei".into();
        ed.move_to(Pos::default(), false);
        ed.find_next(true);
        assert_eq!(ed.selection(), Some((Pos::new(0, 5), Pos::new(0, 9))));
        ed.submit_input(InputKind::ReplaceWith("zwei".into()), "2".into());
        assert_eq!(ed.buf.lines[..2], ["eins 2", "2 drei"]);
        let quit = ed
            .on_key(
                KeyEvent::new(KeyCode::Char('q'), KeyModifiers::CONTROL),
                &mut term,
            )
            .unwrap();
        assert_eq!(quit, Flow::Continue);
        let quit = ed
            .on_key(
                KeyEvent::new(KeyCode::Char('q'), KeyModifiers::CONTROL),
                &mut term,
            )
            .unwrap();
        assert_eq!(quit, Flow::Quit);
        render(&mut ed);
    }
}
