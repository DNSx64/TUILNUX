//! tfiles – Dateibrowser von TUILNUX.

mod fsops;

use std::collections::HashSet;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::{DefaultTerminal, Frame};
use tuilnux_ui::{App, Flow, InputResult, LineInput, frame_block, human_size, keybar, theme};

#[derive(Debug, Clone)]
struct Entry {
    name: String,
    path: PathBuf,
    is_dir: bool,
    is_link: bool,
    size: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InputKind {
    Rename,
    NewFile,
    NewDir,
    Filter,
    Goto,
}

impl InputKind {
    fn label(self) -> &'static str {
        match self {
            InputKind::Rename => " Umbenennen: ",
            InputKind::NewFile => " Neue Datei: ",
            InputKind::NewDir => " Neuer Ordner: ",
            InputKind::Filter => " Filter: ",
            InputKind::Goto => " Gehe zu: ",
        }
    }
}

enum Mode {
    Normal,
    Input(InputKind, LineInput),
    ConfirmDelete(Vec<PathBuf>),
    Help,
}

struct Clipboard {
    paths: Vec<PathBuf>,
    cut: bool,
}

struct Files {
    cwd: PathBuf,
    entries: Vec<Entry>,
    list: ListState,
    show_hidden: bool,
    filter: String,
    marked: HashSet<PathBuf>,
    clipboard: Option<Clipboard>,
    mode: Mode,
    message: Option<(String, bool)>,
    list_area: Rect,
    last_click: Option<(Instant, usize)>,
    preview_key: Option<(PathBuf, u16, u16)>,
    preview: Vec<Line<'static>>,
    preview_wrap: bool,
}

const IMAGE_EXT: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "webp", "bmp", "ico", "tif", "tiff",
];

fn ext_of(path: &Path) -> String {
    path.extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase()
}

fn icon(entry: &Entry) -> &'static str {
    if entry.is_dir {
        return "▸ ";
    }
    match ext_of(&entry.path).as_str() {
        e if IMAGE_EXT.contains(&e) => "▣ ",
        "mp4" | "mkv" | "webm" | "mov" | "avi" => "▶ ",
        "mp3" | "flac" | "ogg" | "wav" | "opus" => "♪ ",
        "zip" | "tar" | "gz" | "xz" | "zst" | "7z" => "◫ ",
        "asciia" => "✦ ",
        "rs" | "py" | "js" | "ts" | "c" | "h" | "cpp" | "go" | "sh" | "toml" | "json" => "≡ ",
        _ => "· ",
    }
}

fn is_text_file(path: &Path) -> bool {
    use std::io::Read;
    let Ok(mut f) = std::fs::File::open(path) else {
        return false;
    };
    let mut buf = [0u8; 4096];
    let Ok(n) = f.read(&mut buf) else {
        return false;
    };
    if buf[..n].contains(&0) {
        return false;
    }
    // Ein am Pufferende abgeschnittenes UTF-8-Zeichen ist kein Hinweis auf Binärdaten.
    match std::str::from_utf8(&buf[..n]) {
        Ok(_) => true,
        Err(e) => e.error_len().is_none(),
    }
}

impl Files {
    fn new(start: PathBuf) -> Self {
        let mut app = Files {
            cwd: start,
            entries: Vec::new(),
            list: ListState::default(),
            show_hidden: false,
            filter: String::new(),
            marked: HashSet::new(),
            clipboard: None,
            mode: Mode::Normal,
            message: None,
            list_area: Rect::default(),
            last_click: None,
            preview_key: None,
            preview: Vec::new(),
            preview_wrap: true,
        };
        app.reload(None);
        app
    }

    fn info(&mut self, msg: impl Into<String>) {
        self.message = Some((msg.into(), false));
    }

    fn error(&mut self, msg: impl Into<String>) {
        self.message = Some((msg.into(), true));
    }

    fn reload(&mut self, select: Option<&Path>) {
        let previous = select
            .map(Path::to_path_buf)
            .or_else(|| self.selected().map(|e| e.path.clone()));
        let filter = self.filter.to_lowercase();
        self.entries = match fsops::read_dir(&self.cwd, self.show_hidden) {
            Ok(list) => list
                .into_iter()
                .map(|(name, path, is_dir, is_link, size)| Entry {
                    name,
                    path,
                    is_dir,
                    is_link,
                    size,
                })
                .filter(|e| filter.is_empty() || e.name.to_lowercase().contains(&filter))
                .collect(),
            Err(e) => {
                self.error(format!("{}: {e}", self.cwd.display()));
                Vec::new()
            }
        };
        let index = previous
            .and_then(|p| self.entries.iter().position(|e| e.path == p))
            .unwrap_or(0);
        let selected = (!self.entries.is_empty()).then(|| index.min(self.entries.len() - 1));
        self.list.select(selected);
        self.marked.retain(|p| p.exists());
        self.preview_key = None;
    }

    fn selected(&self) -> Option<&Entry> {
        self.list.selected().and_then(|i| self.entries.get(i))
    }

    fn targets(&self) -> Vec<PathBuf> {
        if self.marked.is_empty() {
            self.selected()
                .map(|e| vec![e.path.clone()])
                .unwrap_or_default()
        } else {
            let mut v: Vec<PathBuf> = self.marked.iter().cloned().collect();
            v.sort();
            v
        }
    }

    fn change_dir(&mut self, dir: PathBuf) {
        let dir = std::fs::canonicalize(&dir)
            .map(fsops::clean_path)
            .unwrap_or(dir);
        if let Err(e) = std::fs::read_dir(&dir) {
            self.error(format!("{}: {e}", dir.display()));
            return;
        }
        let came_from = std::mem::replace(&mut self.cwd, dir);
        self.filter.clear();
        self.marked.clear();
        self.message = None;
        self.reload(Some(&came_from));
    }

    fn go_parent(&mut self) {
        if let Some(parent) = self.cwd.parent() {
            self.change_dir(parent.to_path_buf());
        }
    }

    fn move_by(&mut self, delta: isize) {
        if self.entries.is_empty() {
            return;
        }
        let cur = self.list.selected().unwrap_or(0) as isize;
        let next = cur
            .saturating_add(delta)
            .clamp(0, self.entries.len() as isize - 1);
        self.list.select(Some(next as usize));
    }

    fn run_tool(
        terminal: &mut DefaultTerminal,
        tool: &str,
        args: &[&std::ffi::OsStr],
    ) -> io::Result<()> {
        tuilnux_ui::suspend(terminal, || {
            print!("\x1b[2J\x1b[H");
            if let Err(e) = std::process::Command::new(tuilnux_ui::tool_path(tool))
                .args(args)
                .status()
            {
                eprintln!("{tool}: {e}");
            }
            tuilnux_ui::wait_for_key("\x1b[2m[ Taste drücken ]\x1b[0m");
        })
    }

    fn activate(&mut self, terminal: &mut DefaultTerminal) -> io::Result<()> {
        let Some(entry) = self.selected().cloned() else {
            return Ok(());
        };
        if entry.is_dir {
            self.change_dir(entry.path);
            return Ok(());
        }
        let ext = ext_of(&entry.path);
        let path = entry.path.as_os_str();
        if IMAGE_EXT.contains(&ext.as_str()) {
            Self::run_tool(terminal, "tview", &[path])?;
        } else if ext == "asciia" {
            Self::run_tool(terminal, "asciia", &[path, "--once".as_ref()])?;
        } else if is_text_file(&entry.path) {
            self.edit(terminal)?;
        } else if let Err(e) = tuilnux_ui::open_with_system(&entry.path) {
            self.error(format!("Öffnen fehlgeschlagen: {e}"));
        } else {
            self.info(format!("Geöffnet mit Standardprogramm: {}", entry.name));
        }
        Ok(())
    }

    fn edit(&mut self, terminal: &mut DefaultTerminal) -> io::Result<()> {
        let Some(entry) = self.selected().cloned() else {
            return Ok(());
        };
        if entry.is_dir {
            return Ok(());
        }
        let status = tuilnux_ui::suspend(terminal, || {
            tuilnux_ui::editor_command().arg(&entry.path).status()
        })?;
        match status {
            Ok(s) if s.success() => self.info(format!("{} bearbeitet", entry.name)),
            Ok(s) => self.error(format!("Editor beendet mit {s}")),
            Err(e) => self.error(format!("Editor konnte nicht gestartet werden: {e}")),
        }
        self.reload(Some(&entry.path));
        Ok(())
    }

    fn paste(&mut self) {
        let Some(clip) = self.clipboard.take() else {
            self.error("Zwischenablage ist leer (c = kopieren, x = ausschneiden)");
            return;
        };
        let mut last = None;
        let mut done = 0;
        let mut failure = None;
        for src in &clip.paths {
            let result = if clip.cut {
                fsops::move_into(src, &self.cwd)
            } else {
                fsops::copy_into(src, &self.cwd)
            };
            match result {
                Ok(dest) => {
                    done += 1;
                    last = Some(dest);
                }
                Err(e) => failure = Some(format!("{}: {e}", src.display())),
            }
        }
        if clip.cut {
            self.marked.clear();
        } else {
            self.clipboard = Some(clip);
        }
        self.reload(last.as_deref());
        match failure {
            Some(msg) => self.error(msg),
            None => self.info(format!("{done} Element(e) eingefügt")),
        }
    }

    fn submit_input(&mut self, kind: InputKind, text: String) {
        let text = text.trim().to_string();
        match kind {
            InputKind::Filter => {
                self.filter = text;
                self.reload(None);
                return;
            }
            InputKind::Goto => {
                if !text.is_empty() {
                    self.change_dir(tuilnux_media::expand_home(&text));
                }
                return;
            }
            _ => {}
        }
        if let Err(msg) = fsops::validate_name(&text) {
            self.error(msg);
            return;
        }
        let target = self.cwd.join(&text);
        let result = match kind {
            InputKind::Rename => match self.selected() {
                Some(e) if target.exists() && e.path != target => Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "Name existiert bereits",
                )),
                Some(e) => std::fs::rename(&e.path, &target),
                None => return,
            },
            InputKind::NewFile => std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&target)
                .map(|_| ()),
            InputKind::NewDir => std::fs::create_dir(&target),
            InputKind::Filter | InputKind::Goto => unreachable!("handled above"),
        };
        match result {
            Ok(()) => {
                self.reload(Some(&target));
                self.info(format!("{text} ✓"));
            }
            Err(e) => self.error(format!("{text}: {e}")),
        }
    }

    fn update_preview(&mut self, area: Rect) {
        let dim = Style::new().fg(theme::dim());
        let Some(entry) = self.selected().cloned() else {
            self.preview = vec![Line::styled("(leer)", dim)];
            return;
        };
        let key = (entry.path.clone(), area.width, area.height);
        if self.preview_key.as_ref() == Some(&key) {
            return;
        }
        self.preview_key = Some(key);
        self.preview_wrap = true;
        let error_line = |msg: String| vec![Line::styled(msg, Style::new().fg(theme::error()))];

        self.preview = if entry.is_dir {
            match fsops::read_dir(&entry.path, self.show_hidden) {
                Ok(children) if children.is_empty() => vec![Line::styled("(leerer Ordner)", dim)],
                Ok(children) => children
                    .into_iter()
                    .take(area.height as usize)
                    .map(|(name, _, is_dir, _, _)| {
                        let style = Style::new().fg(if is_dir {
                            theme::accent2()
                        } else {
                            theme::fg()
                        });
                        Line::styled(format!("{}{name}", if is_dir { "▸ " } else { "  " }), style)
                    })
                    .collect(),
                Err(e) => error_line(e.to_string()),
            }
        } else if IMAGE_EXT.contains(&ext_of(&entry.path).as_str()) {
            match tuilnux_media::load_image(&entry.path) {
                Ok(img) => {
                    self.preview_wrap = false;
                    let (cols, rows) = tuilnux_media::fit_cells(
                        img.width(),
                        img.height(),
                        area.width,
                        area.height.saturating_sub(1),
                    );
                    let mut lines: Vec<Line<'static>> =
                        tuilnux_media::halfblock_cells(&img, cols, rows)
                            .into_iter()
                            .map(|row| {
                                Line::from(
                                    row.into_iter()
                                        .map(|(t, b)| {
                                            Span::styled(
                                                "▀",
                                                Style::new()
                                                    .fg(Color::Rgb(t.0, t.1, t.2))
                                                    .bg(Color::Rgb(b.0, b.1, b.2)),
                                            )
                                        })
                                        .collect::<Vec<_>>(),
                                )
                            })
                            .collect();
                    lines.push(Line::styled(
                        format!(
                            "{}x{} Pixel · {}",
                            img.width(),
                            img.height(),
                            human_size(entry.size)
                        ),
                        dim,
                    ));
                    lines
                }
                Err(e) => error_line(format!("{e:#}")),
            }
        } else if is_text_file(&entry.path) {
            match fsops::read_head(&entry.path, area.height as usize) {
                Ok(lines) => lines
                    .into_iter()
                    .map(|l| Line::styled(l, Style::new().fg(theme::fg())))
                    .collect(),
                Err(e) => error_line(e.to_string()),
            }
        } else {
            vec![
                Line::styled("Binärdatei", dim),
                Line::styled(human_size(entry.size), Style::new().fg(theme::fg())),
                Line::styled("o öffnet mit dem Standardprogramm", dim),
            ]
        };
    }

    fn draw_help(&self, frame: &mut Frame) {
        let rows = [
            ("↑↓ / j k", "auswählen"),
            ("Enter / → / l", "öffnen (Ordner, Bild, Text, Animation)"),
            ("← / h / Backspace", "übergeordneter Ordner"),
            ("~  /  :", "Home  /  Pfad eingeben"),
            ("Leertaste / a", "markieren / alle markieren"),
            ("c / x / v", "kopieren / ausschneiden / einfügen"),
            ("d / Entf", "in den Papierkorb"),
            ("r / F2", "umbenennen"),
            ("n / N", "neue Datei / neuer Ordner"),
            ("e / F4", "im Editor öffnen"),
            ("o", "mit Standardprogramm öffnen"),
            ("/", "filtern"),
            (".", "versteckte Dateien ein/aus"),
            ("R / F5", "aktualisieren"),
            (
                "Maus",
                "Klick wählt, Doppelklick öffnet Ordner, Rad scrollt",
            ),
            ("q / Esc", "beenden"),
        ];
        let area = tuilnux_ui::centered_rect(72, rows.len() as u16 + 2, frame.area());
        let lines: Vec<Line> = rows
            .iter()
            .map(|(k, v)| {
                Line::from(vec![
                    Span::styled(format!(" {k:<20}"), Style::new().fg(theme::accent())),
                    Span::styled(*v, Style::new().fg(theme::fg())),
                ])
            })
            .collect();
        frame.render_widget(Clear, area);
        frame.render_widget(
            Paragraph::new(lines).block(frame_block("Hilfe – tfiles", true)),
            area,
        );
    }

    fn entry_line(&self, e: &Entry, width: usize) -> ListItem<'static> {
        let mark = if self.marked.contains(&e.path) {
            "●"
        } else {
            " "
        };
        let size = if e.is_dir {
            String::from("<DIR>")
        } else {
            human_size(e.size)
        };
        let name = format!("{}{}{}", icon(e), e.name, if e.is_link { " →" } else { "" });
        let name_w = width.saturating_sub(size.len() + 3);
        let shown: String = name.chars().take(name_w).collect();
        let pad = name_w.saturating_sub(shown.chars().count());
        let color = if e.is_dir {
            theme::accent2()
        } else if e.name.starts_with('.') {
            theme::dim()
        } else {
            theme::fg()
        };
        ListItem::new(Line::from(vec![
            Span::styled(mark, Style::new().fg(theme::warn())),
            Span::styled(
                format!("{shown}{}", " ".repeat(pad)),
                Style::new().fg(color),
            ),
            Span::styled(format!(" {size:>8}"), Style::new().fg(theme::dim())),
        ]))
    }
}

impl App for Files {
    fn draw(&mut self, frame: &mut Frame) {
        let [top, main, status, keys] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Min(3),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .areas(frame.area());
        let [left, right] =
            Layout::horizontal([Constraint::Percentage(55), Constraint::Percentage(45)])
                .areas(main);

        let mut path_line = vec![
            Span::styled(
                " tfiles ",
                Style::new().fg(Color::Black).bg(theme::accent()),
            ),
            Span::styled(
                format!(" {}", self.cwd.display()),
                Style::new().fg(theme::fg()).add_modifier(Modifier::BOLD),
            ),
        ];
        if !self.filter.is_empty() {
            path_line.push(Span::styled(
                format!("  [Filter: {}]", self.filter),
                Style::new().fg(theme::warn()),
            ));
        }
        frame.render_widget(Line::from(path_line), top);

        let width = left.width.saturating_sub(4) as usize;
        let items: Vec<ListItem> = self
            .entries
            .iter()
            .map(|e| self.entry_line(e, width))
            .collect();
        let title = format!("{} Einträge", self.entries.len());
        let list = List::new(items)
            .block(frame_block(&title, true))
            .highlight_style(
                Style::new()
                    .bg(theme::sel_bg())
                    .add_modifier(Modifier::BOLD),
            );
        self.list_area = left;
        frame.render_stateful_widget(list, left, &mut self.list);

        let inner = Rect {
            x: right.x + 1,
            y: right.y + 1,
            width: right.width.saturating_sub(2),
            height: right.height.saturating_sub(2),
        };
        self.update_preview(inner);
        let title = self
            .selected()
            .map_or_else(|| "Vorschau".to_string(), |e| e.name.clone());
        let mut preview = Paragraph::new(self.preview.clone()).block(frame_block(&title, false));
        if self.preview_wrap {
            preview = preview.wrap(Wrap { trim: false });
        }
        frame.render_widget(preview, right);

        match &self.mode {
            Mode::Input(kind, input) => input.render(frame, status, kind.label()),
            Mode::ConfirmDelete(paths) => {
                let text = format!(
                    " {} Element(e) in den Papierkorb verschieben? (j/n) ",
                    paths.len()
                );
                frame.render_widget(
                    Line::styled(text, Style::new().fg(Color::Black).bg(theme::warn())),
                    status,
                );
            }
            _ => {
                let (text, color) = match &self.message {
                    Some((m, true)) => (format!(" ✗ {m}"), theme::error()),
                    Some((m, false)) => (format!(" {m}"), theme::accent()),
                    None => {
                        let clip = self.clipboard.as_ref().map_or(String::new(), |c| {
                            let how = if c.cut { "ausschneiden" } else { "kopieren" };
                            format!("  Zwischenablage: {} ({how})", c.paths.len())
                        });
                        let marked = if self.marked.is_empty() {
                            String::new()
                        } else {
                            format!("  markiert: {}", self.marked.len())
                        };
                        (format!(" ? Hilfe{marked}{clip}"), theme::dim())
                    }
                };
                frame.render_widget(
                    Line::styled(text, Style::new().fg(color).bg(theme::bar_bg())),
                    status,
                );
            }
        }
        frame.render_widget(
            keybar(&[
                ("Enter", "Öffnen"),
                ("c", "Kopieren"),
                ("x", "Ausschn."),
                ("v", "Einfügen"),
                ("d", "Löschen"),
                ("r", "Umben."),
                ("n/N", "Neu"),
                ("?", "Hilfe"),
                ("q", "Ende"),
            ]),
            keys,
        );

        if matches!(self.mode, Mode::Help) {
            self.draw_help(frame);
        }
    }

    fn on_key(&mut self, key: KeyEvent, terminal: &mut DefaultTerminal) -> io::Result<Flow> {
        match std::mem::replace(&mut self.mode, Mode::Normal) {
            Mode::Input(kind, mut input) => {
                match input.handle_key(key) {
                    InputResult::Submit => self.submit_input(kind, input.text().to_string()),
                    InputResult::Cancel => {
                        if kind == InputKind::Filter {
                            self.filter.clear();
                            self.reload(None);
                        }
                    }
                    InputResult::Continue => {
                        if kind == InputKind::Filter {
                            self.filter = input.text().to_string();
                            self.reload(None);
                        }
                        self.mode = Mode::Input(kind, input);
                    }
                }
                return Ok(Flow::Continue);
            }
            Mode::ConfirmDelete(paths) => {
                if matches!(
                    key.code,
                    KeyCode::Char('j' | 'y' | 'J' | 'Y') | KeyCode::Enter
                ) {
                    match trash::delete_all(&paths) {
                        Ok(()) => {
                            self.marked.clear();
                            self.reload(None);
                            self.info(format!(
                                "{} Element(e) in den Papierkorb verschoben",
                                paths.len()
                            ));
                        }
                        Err(e) => self.error(format!("Löschen fehlgeschlagen: {e}")),
                    }
                } else {
                    self.info("Abgebrochen");
                }
                return Ok(Flow::Continue);
            }
            Mode::Help => return Ok(Flow::Continue),
            Mode::Normal => {}
        }

        self.message = None;
        let page = self.list_area.height.saturating_sub(2).max(1) as isize;
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => {
                if self.filter.is_empty() {
                    return Ok(Flow::Quit);
                }
                self.filter.clear();
                self.reload(None);
            }
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                return Ok(Flow::Quit);
            }
            KeyCode::Up | KeyCode::Char('k') => self.move_by(-1),
            KeyCode::Down | KeyCode::Char('j') => self.move_by(1),
            KeyCode::PageUp => self.move_by(-page),
            KeyCode::PageDown => self.move_by(page),
            KeyCode::Home | KeyCode::Char('g') => self.move_by(isize::MIN),
            KeyCode::End | KeyCode::Char('G') => self.move_by(isize::MAX),
            KeyCode::Enter | KeyCode::Right | KeyCode::Char('l') => self.activate(terminal)?,
            KeyCode::Left | KeyCode::Backspace | KeyCode::Char('h') => self.go_parent(),
            KeyCode::Char('~') => {
                if let Some(home) = dirs::home_dir() {
                    self.change_dir(home);
                }
            }
            KeyCode::Char('.') => {
                self.show_hidden = !self.show_hidden;
                self.reload(None);
                self.info(if self.show_hidden {
                    "Versteckte Dateien sichtbar"
                } else {
                    "Versteckte Dateien ausgeblendet"
                });
            }
            KeyCode::Char(' ') => {
                if let Some(p) = self.selected().map(|e| e.path.clone()) {
                    if !self.marked.remove(&p) {
                        self.marked.insert(p);
                    }
                    self.move_by(1);
                }
            }
            KeyCode::Char('a') => {
                if self.marked.len() == self.entries.len() {
                    self.marked.clear();
                } else {
                    self.marked = self.entries.iter().map(|e| e.path.clone()).collect();
                }
            }
            KeyCode::Char('c' | 'y' | 'x') => {
                let cut = key.code == KeyCode::Char('x');
                let paths = self.targets();
                if !paths.is_empty() {
                    let how = if cut { "ausgeschnitten" } else { "kopiert" };
                    self.info(format!("{} Element(e) {how} – mit v einfügen", paths.len()));
                    self.clipboard = Some(Clipboard { paths, cut });
                }
            }
            KeyCode::Char('v' | 'p') => self.paste(),
            KeyCode::Char('d') | KeyCode::Delete => {
                let paths = self.targets();
                if !paths.is_empty() {
                    self.mode = Mode::ConfirmDelete(paths);
                }
            }
            KeyCode::Char('r') | KeyCode::F(2) => {
                if let Some(name) = self.selected().map(|e| e.name.clone()) {
                    self.mode = Mode::Input(InputKind::Rename, LineInput::new(name));
                }
            }
            KeyCode::Char('n') => self.mode = Mode::Input(InputKind::NewFile, LineInput::default()),
            KeyCode::Char('N') | KeyCode::F(7) => {
                self.mode = Mode::Input(InputKind::NewDir, LineInput::default())
            }
            KeyCode::Char('/') => {
                self.mode = Mode::Input(InputKind::Filter, LineInput::new(self.filter.clone()))
            }
            KeyCode::Char(':') => {
                self.mode = Mode::Input(
                    InputKind::Goto,
                    LineInput::new(self.cwd.display().to_string()),
                )
            }
            KeyCode::Char('e') | KeyCode::F(4) => self.edit(terminal)?,
            KeyCode::Char('o') => {
                if let Some(p) = self.selected().map(|e| e.path.clone()) {
                    match tuilnux_ui::open_with_system(&p) {
                        Ok(()) => self.info("Mit Standardprogramm geöffnet"),
                        Err(e) => self.error(e.to_string()),
                    }
                }
            }
            KeyCode::Char('R') | KeyCode::F(5) => {
                self.reload(None);
                self.info("Aktualisiert");
            }
            KeyCode::Char('?') | KeyCode::F(1) => self.mode = Mode::Help,
            _ => {}
        }
        Ok(Flow::Continue)
    }

    fn on_mouse(&mut self, mouse: MouseEvent) -> Flow {
        if !matches!(self.mode, Mode::Normal) {
            return Flow::Continue;
        }
        let area = self.list_area;
        let inside = mouse.column > area.x
            && mouse.column + 1 < area.right()
            && mouse.row > area.y
            && mouse.row + 1 < area.bottom();
        match mouse.kind {
            MouseEventKind::ScrollUp => self.move_by(-3),
            MouseEventKind::ScrollDown => self.move_by(3),
            MouseEventKind::Down(MouseButton::Left) if inside => {
                let index = self.list.offset() + (mouse.row - area.y - 1) as usize;
                if index < self.entries.len() {
                    let double = self.last_click.is_some_and(|(t, i)| {
                        i == index && t.elapsed() < Duration::from_millis(450)
                    });
                    self.list.select(Some(index));
                    self.last_click = (!double).then(|| (Instant::now(), index));
                    if double {
                        match self.selected().cloned() {
                            Some(e) if e.is_dir => self.change_dir(e.path),
                            Some(_) => self.info("Enter öffnet die Datei"),
                            None => {}
                        }
                    }
                }
            }
            MouseEventKind::Down(MouseButton::Right) => self.go_parent(),
            _ => {}
        }
        Flow::Continue
    }
}

fn main() -> anyhow::Result<()> {
    let start = match std::env::args().nth(1) {
        Some(arg) if arg == "-h" || arg == "--help" => {
            println!("tfiles [ordner] – Dateibrowser von TUILNUX (? zeigt alle Tasten)");
            return Ok(());
        }
        Some(arg) => tuilnux_media::expand_home(&arg),
        None => std::env::current_dir()?,
    };
    let start = std::fs::canonicalize(&start).map(fsops::clean_path)?;
    let mut app = Files::new(start);
    tuilnux_ui::run(&mut app)?;
    Ok(())
}
