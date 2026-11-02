//! tsettings – Einstellungen von TUILNUX.
//!
//! Bearbeitet `~/.config/tuilnux/tuilnux.toml` und das Hintergrundbild, schreibt beim
//! Speichern die Dateien für labwc, GTK und die Sitzung und lädt die laufende
//! TUILNUX-Sitzung live neu.

mod fields;

use std::io;
use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use fields::{Field, Kind, Model, Page, Shown};
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{List, ListItem, ListState, Paragraph};
use ratatui::{DefaultTerminal, Frame};
use tuilnux_config::Settings;
use tuilnux_ipc::{Request, Response};
use tuilnux_media::WallpaperConfig;
use tuilnux_ui::{
    App, Flow, InputResult, LineInput, frame_block, is_ctrl, keybar, status_line, theme, tr,
};

const HELP: &str = "\
tsettings – Einstellungen von TUILNUX

Aufruf: tsettings [BEREICH]
  BEREICH  Seite, mit der begonnen wird (z. B. maus, gui, tastatur)

Bedienung:
  Tab/Shift+Tab, ←       Bereich wechseln
  ↑ ↓                    Einstellung wählen
  ← → / Leertaste        Wert ändern
  Enter                  Wert eingeben (Text, Zahl, Farbe #rrggbb)
  r                      Einstellung auf Standard zurücksetzen
  Strg+S                 speichern und anwenden (lädt TUILNUX und labwc neu)
  q / Esc                beenden
Datei: ~/.config/tuilnux/tuilnux.toml";

const MESSAGE_TIME: Duration = Duration::from_secs(6);
const LABEL_WIDTH: usize = 22;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pane {
    Pages,
    Fields,
}

struct Message {
    text: String,
    error: bool,
    at: Instant,
}

struct SettingsApp {
    pages: Vec<Page>,
    model: Model,
    saved: Model,
    page: usize,
    focus: Pane,
    fields: ListState,
    input: Option<LineInput>,
    confirm_quit: bool,
    message: Option<Message>,
    pages_rect: Rect,
    fields_rect: Rect,
}

impl SettingsApp {
    fn new(model: Model) -> Self {
        theme::set(&model.s);
        Self {
            pages: fields::pages(&model),
            saved: model.clone(),
            model,
            page: 0,
            focus: Pane::Pages,
            fields: ListState::default().with_selected(Some(0)),
            input: None,
            confirm_quit: false,
            message: None,
            pages_rect: Rect::default(),
            fields_rect: Rect::default(),
        }
    }

    fn dirty(&self) -> bool {
        self.model != self.saved
    }

    fn page(&self) -> &Page {
        &self.pages[self.page]
    }

    fn field_index(&self) -> usize {
        self.fields
            .selected()
            .unwrap_or(0)
            .min(self.page().fields.len().saturating_sub(1))
    }

    fn field(&self) -> &Field {
        &self.page().fields[self.field_index()]
    }

    fn notify(&mut self, text: impl Into<String>, error: bool) {
        self.message = Some(Message {
            text: text.into(),
            error,
            at: Instant::now(),
        });
    }

    fn select_page(&mut self, page: usize) {
        self.page = page.min(self.pages.len() - 1);
        self.fields = ListState::default().with_selected(Some(0));
    }

    fn move_page(&mut self, delta: isize) {
        let n = self.pages.len() as isize;
        self.select_page((self.page as isize + delta).rem_euclid(n) as usize);
    }

    fn move_field(&mut self, delta: isize) {
        let n = self.page().fields.len() as isize;
        let i = (self.field_index() as isize + delta).clamp(0, n - 1);
        self.fields.select(Some(i as usize));
    }

    /// Ändert das Modell über `f` und nimmt die Änderung zurück, wenn sie ungültig ist.
    fn change(&mut self, f: impl FnOnce(&Field, &mut Model) -> Result<(), String>) {
        let before = self.model.clone();
        let field = &self.pages[self.page].fields[self.field_index()];
        let result = f(field, &mut self.model).and_then(|()| self.model.validate());
        match result {
            Ok(()) => {
                if self.model.s.appearance != before.s.appearance
                    || self.model.s.windows.ascii != before.s.windows.ascii
                {
                    theme::set(&self.model.s);
                }
            }
            Err(e) => {
                self.model = before;
                self.notify(e, true);
            }
        }
    }

    fn step(&mut self, dir: i32) {
        self.change(|f, m| {
            f.step(m, dir);
            Ok(())
        });
    }

    fn reset_field(&mut self) {
        self.change(|f, m| {
            f.reset(m);
            Ok(())
        });
    }

    fn activate(&mut self) {
        let field = self.field();
        if !field.editable() {
            return;
        }
        if let Kind::Action { run, .. } = field.kind {
            match run(&mut self.model) {
                Ok(text) => self.notify(text, false),
                Err(e) => self.notify(e, true),
            }
            return;
        }
        if field.needs_input() {
            self.input = Some(LineInput::new(field.input_text(&self.model)));
        } else {
            self.step(1);
        }
    }

    fn submit_input(&mut self, text: String) {
        self.change(|f, m| f.submit(m, &text));
    }

    fn save(&mut self) -> bool {
        if let Err(e) = self.model.validate() {
            self.notify(e, true);
            return false;
        }
        let applied = match tuilnux_config::apply(&self.model.s, Some(&self.saved.s)) {
            Ok(a) => a,
            Err(e) => {
                self.notify(format!("Speichern fehlgeschlagen: {e:#}"), true);
                return false;
            }
        };
        if self.model.wp != self.saved.wp
            && let Err(e) = self.model.wp.save()
        {
            self.notify(format!("Hintergrundbild: {e:#}"), true);
            return false;
        }
        let mut notes = vec![format!("Gespeichert ({} Dateien)", applied.written.len())];
        match tuilnux_ipc::request_env(Request::Reload) {
            Ok(Response::Ok) => notes.push("TUILNUX neu geladen".into()),
            Ok(Response::Error { message }) => notes.push(format!("TUILNUX: {message}")),
            Ok(_) => {}
            Err(_) => notes.push("keine laufende Sitzung – wirkt beim nächsten Start".into()),
        }
        notes.extend(applied.notes);
        self.saved = self.model.clone();
        self.notify(notes.join(" · "), false);
        true
    }

    fn quit(&mut self) -> Flow {
        if self.dirty() {
            self.confirm_quit = true;
            Flow::Continue
        } else {
            Flow::Quit
        }
    }

    fn key(&mut self, key: KeyEvent) -> Flow {
        if let Some(input) = &mut self.input {
            match input.handle_key(key) {
                InputResult::Continue => {}
                InputResult::Cancel => self.input = None,
                InputResult::Submit => {
                    let text = input.text().to_string();
                    self.input = None;
                    self.submit_input(text);
                }
            }
            return Flow::Continue;
        }
        if self.confirm_quit {
            self.confirm_quit = false;
            return match key.code {
                KeyCode::Char('s' | 'j' | 'y') => {
                    if self.save() {
                        Flow::Quit
                    } else {
                        Flow::Continue
                    }
                }
                KeyCode::Char('v' | 'n') => Flow::Quit,
                _ => Flow::Continue,
            };
        }
        if is_ctrl(&key, 's') {
            self.save();
            return Flow::Continue;
        }
        if is_ctrl(&key, 'q') || is_ctrl(&key, 'c') {
            return self.quit();
        }
        match key.code {
            KeyCode::Char('q') => return self.quit(),
            KeyCode::Esc if self.focus == Pane::Fields => self.focus = Pane::Pages,
            KeyCode::Esc => return self.quit(),
            KeyCode::Tab => self.move_page(1),
            KeyCode::BackTab => self.move_page(-1),
            _ => match self.focus {
                Pane::Pages => self.pages_key(key),
                Pane::Fields => self.fields_key(key),
            },
        }
        Flow::Continue
    }

    fn pages_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => self.move_page(-1),
            KeyCode::Down | KeyCode::Char('j') => self.move_page(1),
            KeyCode::Home => self.select_page(0),
            KeyCode::End => self.select_page(self.pages.len() - 1),
            KeyCode::Right | KeyCode::Enter | KeyCode::Char(' ' | 'l') => self.focus = Pane::Fields,
            _ => {}
        }
    }

    fn fields_key(&mut self, key: KeyEvent) {
        let steppable = matches!(
            self.field().kind,
            Kind::Bool { .. } | Kind::Choice { .. } | Kind::Number { .. }
        );
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => self.move_field(-1),
            KeyCode::Down | KeyCode::Char('j') => self.move_field(1),
            KeyCode::Home => self.fields.select(Some(0)),
            KeyCode::End => self.move_field(isize::MAX / 2),
            KeyCode::PageUp => self.move_field(-10),
            KeyCode::PageDown => self.move_field(10),
            KeyCode::Left | KeyCode::Char('h') if steppable => self.step(-1),
            KeyCode::Right | KeyCode::Char('l') if steppable => self.step(1),
            KeyCode::Left | KeyCode::Char('h') => self.focus = Pane::Pages,
            KeyCode::Char('+') if steppable => self.step(1),
            KeyCode::Char('-') if steppable => self.step(-1),
            KeyCode::Char(' ') if !self.field().needs_input() => self.step(1),
            KeyCode::Enter => self.activate(),
            KeyCode::Char('r') if self.field().editable() => self.reset_field(),
            _ => {}
        }
    }

    fn mouse(&mut self, mouse: MouseEvent) {
        if self.input.is_some() || self.confirm_quit {
            return;
        }
        let pos = Position::new(mouse.column, mouse.row);
        let inner = |r: Rect| Rect {
            x: r.x + 1,
            y: r.y + 1,
            width: r.width.saturating_sub(2),
            height: r.height.saturating_sub(2),
        };
        let pages = inner(self.pages_rect);
        let list = self.fields_rect;
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) if pages.contains(pos) => {
                let row = (mouse.row - pages.y) as usize;
                if row < self.pages.len() {
                    self.select_page(row);
                    self.focus = Pane::Pages;
                }
            }
            MouseEventKind::Down(MouseButton::Left) if list.contains(pos) => {
                let row = self.fields.offset() + (mouse.row - list.y) as usize;
                if row < self.page().fields.len() {
                    let again = self.focus == Pane::Fields && self.field_index() == row;
                    self.focus = Pane::Fields;
                    self.fields.select(Some(row));
                    // Klick auf den Wert oder zweiter Klick: ändern.
                    let on_value = mouse.column >= list.x + LABEL_WIDTH as u16 + 2;
                    if again || on_value {
                        self.activate();
                    }
                }
            }
            MouseEventKind::Down(MouseButton::Right) if list.contains(pos) => {
                let row = self.fields.offset() + (mouse.row - list.y) as usize;
                if row < self.page().fields.len() {
                    self.focus = Pane::Fields;
                    self.fields.select(Some(row));
                    if !self.field().needs_input() {
                        self.step(-1);
                    }
                }
            }
            MouseEventKind::ScrollUp if pages.contains(pos) => self.move_page(-1),
            MouseEventKind::ScrollDown if pages.contains(pos) => self.move_page(1),
            MouseEventKind::ScrollUp => self.move_field(-1),
            MouseEventKind::ScrollDown => self.move_field(1),
            _ => {}
        }
    }

    fn value_spans(&self, field: &Field) -> Vec<Span<'static>> {
        let dim = Style::new().fg(theme::dim());
        let steppable = matches!(field.kind, Kind::Choice { .. } | Kind::Number { .. });
        match field.shown(&self.model) {
            Shown::Bool(true) => vec![Span::styled(
                tr("[x] an").to_string(),
                Style::new().fg(theme::accent()),
            )],
            Shown::Bool(false) => vec![Span::styled(tr("[ ] aus").to_string(), dim)],
            Shown::Text(t) if steppable => vec![
                Span::styled("‹ ", dim),
                Span::styled(t, Style::new().fg(theme::fg())),
                Span::styled(" ›", dim),
            ],
            Shown::Text(t) => vec![Span::styled(t, Style::new().fg(theme::fg()))],
            Shown::Color { rgb, custom } => {
                let mut spans = vec![
                    Span::styled("██", Style::new().fg(theme::rgb(rgb))),
                    Span::styled(
                        format!(" {}", tuilnux_config::hex(rgb)),
                        Style::new().fg(theme::fg()),
                    ),
                ];
                if !custom {
                    spans.push(Span::styled(tr(" (Schema)").to_string(), dim));
                }
                spans
            }
            Shown::Info(t) => vec![Span::styled(t, dim)],
        }
    }

    fn draw_preview(&self, frame: &mut Frame, area: Rect) {
        let sample = |text: &'static str, color| Span::styled(text, Style::new().fg(color));
        let lines = vec![
            Line::from(vec![
                sample("Text ", theme::fg()),
                sample("Akzent ", theme::accent()),
                sample("Zweitakzent ", theme::accent2()),
                sample("gedimmt ", theme::dim()),
                sample("Warnung ", theme::warn()),
                sample("Fehler", theme::error()),
            ]),
            Line::from(Span::styled(
                " ausgewählte Zeile ",
                Style::new().bg(theme::sel_bg()).fg(theme::accent()),
            )),
            Line::from(vec![
                Span::styled(
                    format!(" {} ", self.model.s.bar.label),
                    Style::new().bg(theme::accent()).fg(theme::BG),
                ),
                Span::styled(
                    " 1:Dateien  2:Firefox   CPU 12%  RAM 41%  12:00 ",
                    Style::new().bg(theme::statusbar()).fg(theme::fg()),
                ),
            ]),
        ];
        frame.render_widget(
            Paragraph::new(lines).block(frame_block(tr("Vorschau"), false)),
            area,
        );
    }

    fn draw_prompt_preview(&self, frame: &mut Frame, area: Rect) {
        let mut lines = vec![Line::default()];
        for (text, color) in tuilnux_config::prompt_preview(&self.model.s) {
            let style = color.map_or(Style::new().fg(theme::fg()), |c| {
                Style::new().fg(theme::rgb(c))
            });
            let mut parts = text.split('\n');
            if let Some(first) = parts.next() {
                lines
                    .last_mut()
                    .unwrap()
                    .push_span(Span::styled(first.to_string(), style));
            }
            for part in parts {
                lines.push(Line::from(Span::styled(part.to_string(), style)));
            }
        }
        lines
            .last_mut()
            .unwrap()
            .push_span(Span::styled("ls -la", Style::new().fg(theme::fg())));
        frame.render_widget(
            Paragraph::new(lines).block(frame_block(tr("Vorschau Prompt"), false)),
            area,
        );
    }

    fn render(&mut self, frame: &mut Frame) {
        let area = frame.area();
        let [header, body, help, bottom] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Fill(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .areas(area);

        let state = if self.dirty() {
            tr("● ungespeichert  ")
        } else {
            tr("gespeichert  ")
        };
        frame.render_widget(
            status_line(
                tr(" TUILNUX Einstellungen").into(),
                state.into(),
                header.width,
            ),
            header,
        );

        // Breite nach dem längsten (übersetzten) Seitentitel: Rahmen 2 + Zeiger 1 + Leerzeichen 1.
        let side = self
            .pages
            .iter()
            .map(|p| tr(p.title).chars().count())
            .max()
            .unwrap_or(16)
            .clamp(16, 28) as u16
            + 4;
        let [left, right] =
            Layout::horizontal([Constraint::Length(side), Constraint::Fill(1)]).areas(body);
        self.pages_rect = left;
        let items: Vec<ListItem> = self
            .pages
            .iter()
            .map(|p| ListItem::new(format!(" {}", tr(p.title))))
            .collect();
        let mut page_state = ListState::default().with_selected(Some(self.page));
        let highlight = if self.focus == Pane::Pages {
            Style::new()
                .bg(theme::sel_bg())
                .fg(theme::accent())
                .add_modifier(Modifier::BOLD)
        } else {
            Style::new().fg(theme::accent())
        };
        frame.render_stateful_widget(
            List::new(items)
                .block(frame_block(tr("Bereiche"), self.focus == Pane::Pages))
                .highlight_style(highlight)
                .highlight_symbol("›"),
            left,
            &mut page_state,
        );

        let page = &self.pages[self.page];
        let block = frame_block(tr(page.title), self.focus == Pane::Fields);
        let inner = block.inner(right);
        frame.render_widget(block, right);
        let prompt_preview = page.title == "Shell & Prompt";
        let preview = (self.page == 0 || prompt_preview) && inner.height > 12;
        let [intro, list, preview_area] = Layout::vertical([
            Constraint::Length(2),
            Constraint::Fill(1),
            Constraint::Length(if preview { 5 } else { 0 }),
        ])
        .areas(inner);
        frame.render_widget(
            Paragraph::new(Span::styled(tr(page.help), Style::new().fg(theme::dim())))
                .wrap(ratatui::widgets::Wrap { trim: true }),
            intro,
        );
        self.fields_rect = list;
        let items: Vec<ListItem> = page
            .fields
            .iter()
            .map(|f| {
                let changed = f.shown(&self.model) != f.shown(&self.saved);
                let mut spans = vec![
                    Span::styled(
                        if changed { "● " } else { "  " },
                        Style::new().fg(theme::accent2()),
                    ),
                    Span::styled(
                        format!("{:<LABEL_WIDTH$}", tr(f.label)),
                        Style::new().fg(theme::fg()),
                    ),
                ];
                spans.extend(self.value_spans(f));
                ListItem::new(Line::from(spans))
            })
            .collect();
        let selected = if self.focus == Pane::Fields {
            Style::new()
                .bg(theme::sel_bg())
                .add_modifier(Modifier::BOLD)
        } else {
            Style::new()
        };
        let index = self.field_index();
        self.fields.select(Some(index));
        frame.render_stateful_widget(
            List::new(items).highlight_style(selected),
            list,
            &mut self.fields,
        );
        if preview && prompt_preview {
            self.draw_prompt_preview(frame, preview_area);
        } else if preview {
            self.draw_preview(frame, preview_area);
        }

        let help_line = match &self.message {
            Some(m) => Line::from(Span::styled(
                format!(" {}", tr(&m.text)),
                Style::new().fg(if m.error {
                    theme::error()
                } else {
                    theme::accent()
                }),
            )),
            None => Line::from(Span::styled(
                format!(" {}", tr(self.field().help)),
                Style::new().fg(theme::dim()),
            )),
        };
        frame.render_widget(Paragraph::new(help_line), help);

        if let Some(input) = &self.input {
            input.render(frame, bottom, &format!(" {}: ", tr(self.field().label)));
        } else if self.confirm_quit {
            frame.render_widget(
                keybar(&[("s", "speichern"), ("v", "verwerfen"), ("Esc", "zurück")]),
                bottom,
            );
        } else {
            frame.render_widget(
                keybar(&[
                    ("↑↓", "wählen"),
                    ("←→", "ändern"),
                    ("Enter", "eingeben"),
                    ("Tab", "Bereich"),
                    ("r", "Standard"),
                    ("^S", "speichern"),
                    ("q", "beenden"),
                ]),
                bottom,
            );
        }
    }
}

impl App for SettingsApp {
    fn draw(&mut self, frame: &mut Frame) {
        self.render(frame);
    }

    fn on_key(&mut self, key: KeyEvent, _term: &mut DefaultTerminal) -> io::Result<Flow> {
        Ok(self.key(key))
    }

    fn on_mouse(&mut self, mouse: MouseEvent) -> Flow {
        self.mouse(mouse);
        Flow::Continue
    }

    fn on_paste(&mut self, text: String) {
        if let Some(input) = &mut self.input {
            for c in text.chars().filter(|c| !c.is_control()) {
                input.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
            }
        }
    }

    fn tick(&mut self) {
        if self
            .message
            .as_ref()
            .is_some_and(|m| m.at.elapsed() > MESSAGE_TIME)
        {
            self.message = None;
        }
    }
}

fn find_page(pages: &[Page], name: &str) -> Option<usize> {
    let name = name.to_lowercase();
    pages
        .iter()
        .position(|p| p.title.to_lowercase().starts_with(&name))
}

fn main() -> anyhow::Result<()> {
    let arg = std::env::args().nth(1);
    if matches!(arg.as_deref(), Some("-h" | "--help")) {
        println!("{HELP}");
        return Ok(());
    }
    let (settings, warning) = match Settings::load() {
        Ok(s) => (s, None),
        Err(e) => (
            Settings::default(),
            Some(format!(
                "{e:#} – Standardwerte geladen, Speichern überschreibt die Datei"
            )),
        ),
    };
    let wp = WallpaperConfig::load().unwrap_or_default();
    let outputs = tuilnux_config::detect_outputs().unwrap_or_default();
    let gpus = tuilnux_config::detect_gpus();
    let mut app = SettingsApp::new(Model {
        s: settings,
        wp,
        outputs,
        gpus,
    });
    if let Some(name) = arg {
        match find_page(&app.pages, &name) {
            Some(i) => {
                app.select_page(i);
                app.focus = Pane::Fields;
            }
            None => anyhow::bail!("unbekannter Bereich: {name} (siehe tsettings --help)"),
        }
    }
    if let Some(w) = warning {
        app.notify(w, true);
    }
    tuilnux_ui::run(&mut app)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use std::sync::{Mutex, MutexGuard};

    /// Das Farbschema ist global; Tests, die es setzen, laufen nacheinander.
    fn lock() -> MutexGuard<'static, ()> {
        static LOCK: Mutex<()> = Mutex::new(());
        LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn press(app: &mut SettingsApp, code: KeyCode) -> Flow {
        app.key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    fn screen(app: &mut SettingsApp) -> String {
        let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();
        term.draw(|f| app.render(f)).unwrap();
        let buf = term.backend().buffer();
        (0..buf.area.height)
            .map(|y| {
                (0..buf.area.width)
                    .map(|x| buf[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn keyboard_navigation_and_editing() {
        let _guard = lock();
        let mut app = SettingsApp::new(Model::default());
        assert!(screen(&mut app).contains("Farbschema"));
        press(&mut app, KeyCode::Down);
        assert_eq!(app.page().title, "Statusleiste");
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.focus, Pane::Fields);
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Char(' '));
        assert!(!app.model.s.bar.mouse);
        assert!(app.dirty());
        assert!(screen(&mut app).contains("● ungespeichert"));
        press(&mut app, KeyCode::Char('r'));
        assert!(app.model.s.bar.mouse);
        assert!(!app.dirty());

        // Uhrformat eingeben: ungültig wird abgelehnt.
        app.fields.select(Some(7));
        press(&mut app, KeyCode::Enter);
        assert!(app.input.is_some());
        app.key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
        for c in "%Q".chars() {
            press(&mut app, KeyCode::Char(c));
        }
        press(&mut app, KeyCode::Enter);
        assert!(app.input.is_none());
        assert!(app.message.as_ref().unwrap().error);
        assert_eq!(app.model.s.bar.clock_format, "%Y-%m-%d %H:%M:%S");

        // Ungespeichert beenden fragt nach.
        press(&mut app, KeyCode::Char(' '));
        app.fields.select(Some(2));
        press(&mut app, KeyCode::Right);
        assert!(!app.model.s.bar.cpu);
        assert_eq!(press(&mut app, KeyCode::Char('q')), Flow::Continue);
        assert!(app.confirm_quit);
        assert!(screen(&mut app).contains("verwerfen"));
        assert_eq!(press(&mut app, KeyCode::Char('v')), Flow::Quit);
    }

    #[test]
    fn appearance_changes_preview_live() {
        let _guard = lock();
        let mut app = SettingsApp::new(Model::default());
        app.focus = Pane::Fields;
        press(&mut app, KeyCode::Right);
        assert_eq!(
            app.model.s.appearance.theme,
            tuilnux_config::ThemePreset::Nord
        );
        let nord = tuilnux_config::ThemePreset::Nord.palette();
        assert_eq!(theme::accent(), theme::rgb(nord.accent));
        let text = screen(&mut app);
        assert!(text.contains("Vorschau") && text.contains("Nord"));
        press(&mut app, KeyCode::Left);
        assert!(!app.dirty());
    }

    #[test]
    fn every_page_renders() {
        let _guard = lock();
        let mut app = SettingsApp::new(Model::default());
        for i in 0..app.pages.len() {
            app.select_page(i);
            let text = screen(&mut app);
            assert!(text.contains(app.pages[i].title));
        }
        assert_eq!(find_page(&app.pages, "maus"), Some(6));
        assert_eq!(find_page(&app.pages, "GUI"), Some(3));
        assert_eq!(find_page(&app.pages, "xyz"), None);
    }

    #[test]
    fn mouse_selects_and_changes() {
        let _guard = lock();
        let mut app = SettingsApp::new(Model::default());
        screen(&mut app);
        let click = |app: &mut SettingsApp, kind, column, row| {
            app.mouse(MouseEvent {
                kind,
                column,
                row,
                modifiers: KeyModifiers::NONE,
            });
        };
        // Seite „Maus“ in der linken Liste (Zeile 2 = erster Eintrag).
        click(&mut app, MouseEventKind::Down(MouseButton::Left), 3, 2 + 6);
        assert_eq!(app.page().title, "Maus");
        screen(&mut app);
        let list = app.fields_rect;
        // Wert von „Natürliches Scrollen“ anklicken.
        click(
            &mut app,
            MouseEventKind::Down(MouseButton::Left),
            list.x + 30,
            list.y + 2,
        );
        assert!(app.model.s.mouse.natural_scroll);
        click(&mut app, MouseEventKind::ScrollDown, list.x + 5, list.y);
        assert_eq!(app.field_index(), 3);
    }
}
