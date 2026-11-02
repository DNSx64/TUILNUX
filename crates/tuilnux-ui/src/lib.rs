//! Gemeinsame Bausteine für alle TUILNUX-Apps: Theme, Rahmen, Tastenleiste,
//! Texteingabe und eine einheitliche Ereignisschleife.

use std::io::{self, stdout};
use std::time::{Duration, Instant};

use crossterm::event::{
    self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseEvent,
};
use crossterm::execute;
use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders};
use ratatui::{DefaultTerminal, Frame};

mod prefix;
pub use prefix::Prefix;
pub use tuilnux_config;
pub use tuilnux_i18n::{self as i18n, tr, trf};

/// Farben und Rahmen aus `~/.config/tuilnux/tuilnux.toml` (Abschnitt `[appearance]`).
/// Beim ersten Zugriff geladen; [`theme::reload`] liest die Datei neu.
pub mod theme {
    use std::sync::{LazyLock, RwLock};

    use ratatui::style::Color;
    use ratatui::widgets::BorderType;
    use tuilnux_config::{Borders, Palette, Rgb, Settings};

    #[derive(Clone, Copy)]
    struct Theme {
        palette: Palette,
        borders: Borders,
    }

    impl Theme {
        fn from(settings: &Settings) -> Self {
            Self {
                palette: settings.palette(),
                borders: settings.appearance.borders,
            }
        }
    }

    static THEME: LazyLock<RwLock<Theme>> =
        LazyLock::new(|| RwLock::new(Theme::from(&Settings::load_or_default())));

    fn get() -> Theme {
        *THEME.read().unwrap_or_else(|e| e.into_inner())
    }

    /// Übernimmt Farben und Rahmen aus `settings` (z. B. nach einer Änderung in tsettings).
    pub fn set(settings: &Settings) {
        tuilnux_i18n::init(&settings.language);
        *THEME.write().unwrap_or_else(|e| e.into_inner()) = Theme::from(settings);
    }

    /// Liest die Einstellungsdatei neu und gibt sie zurück.
    pub fn reload() -> Settings {
        let settings = Settings::load_or_default();
        set(&settings);
        settings
    }

    pub fn rgb(c: Rgb) -> Color {
        Color::Rgb(c[0], c[1], c[2])
    }

    pub const BG: Color = Color::Reset;

    pub fn fg() -> Color {
        rgb(get().palette.fg)
    }
    pub fn dim() -> Color {
        rgb(get().palette.dim)
    }
    pub fn accent() -> Color {
        rgb(get().palette.accent)
    }
    pub fn accent2() -> Color {
        rgb(get().palette.accent2)
    }
    pub fn warn() -> Color {
        rgb(get().palette.warn)
    }
    pub fn error() -> Color {
        rgb(get().palette.error)
    }
    /// Hintergrund markierter Einträge.
    pub fn sel_bg() -> Color {
        rgb(get().palette.selection)
    }
    /// Hintergrund von Tasten- und Statuszeilen der Apps.
    pub fn bar_bg() -> Color {
        rgb(get().palette.panel)
    }
    /// Hintergrund der TUILNUX-Statusleiste.
    pub fn statusbar() -> Color {
        rgb(get().palette.statusbar)
    }

    pub fn border_type() -> BorderType {
        match get().borders {
            Borders::Plain => BorderType::Plain,
            Borders::Rounded => BorderType::Rounded,
            Borders::Double => BorderType::Double,
            Borders::Thick => BorderType::Thick,
        }
    }

    pub fn borders() -> Borders {
        get().borders
    }
}

/// Umgebungsvariable, mit der sich der ASCII-Ersatz für Rahmenlinien erzwingen
/// (`1`) oder abschalten (`0`) lässt, z. B. bei einer Unicode-Konsolenschrift.
pub const ASCII_ENV: &str = "TUILNUX_ASCII";

/// Nur ASCII-Rahmen zeichnen? Standard: ja auf der Linux-Konsole (`TERM=linux`),
/// deren eingebaute Schrift nicht alle Linien kennt.
pub fn ascii_console() -> bool {
    ascii_for(
        std::env::var(ASCII_ENV).ok().as_deref(),
        std::env::var("TERM").ok().as_deref(),
    )
}

fn ascii_for(force: Option<&str>, term: Option<&str>) -> bool {
    match force.map(str::trim) {
        Some("1" | "yes" | "true" | "on") => true,
        Some("0" | "no" | "false" | "off") => false,
        _ => term == Some("linux"),
    }
}

/// Rahmen im TUILNUX-Terminal-Stil mit Titel `[ titel ]`.
pub fn frame_block(title: &str, focused: bool) -> Block<'_> {
    let color = if focused {
        theme::accent()
    } else {
        theme::dim()
    };
    Block::default()
        .borders(Borders::ALL)
        .border_type(theme::border_type())
        .border_style(Style::new().fg(color))
        .title(Line::from(vec![
            Span::styled("[ ", Style::new().fg(color)),
            Span::styled(
                tr(title),
                Style::new().fg(theme::fg()).add_modifier(Modifier::BOLD),
            ),
            Span::styled(" ]", Style::new().fg(color)),
        ]))
}

/// Untere Tastenleiste wie bei mc/nano: `^S Speichern  ^Q Beenden`.
pub fn keybar<'a>(items: &[(&'a str, &'a str)]) -> Line<'a> {
    let mut spans = Vec::with_capacity(items.len() * 3);
    for (key, label) in items {
        spans.push(Span::styled(
            format!(" {key} "),
            Style::new().fg(Color::Black).bg(theme::accent()),
        ));
        spans.push(Span::styled(
            format!(" {} ", tr(label)),
            Style::new().fg(theme::fg()),
        ));
    }
    Line::from(spans).style(Style::new().bg(theme::bar_bg()))
}

/// Statuszeile: links Text, rechts Text, gemeinsamer Hintergrund.
pub fn status_line<'a>(left: String, right: String, width: u16) -> Line<'a> {
    let right_len = right.chars().count();
    let room = (width as usize).saturating_sub(right_len);
    let left: String = if left.chars().count() > room {
        let mut s: String = left.chars().take(room.saturating_sub(1)).collect();
        s.push('…');
        s
    } else {
        left
    };
    let used = left.chars().count() + right_len;
    let pad = (width as usize).saturating_sub(used);
    Line::from(vec![
        Span::styled(left, Style::new().fg(theme::accent())),
        Span::raw(" ".repeat(pad)),
        Span::styled(right, Style::new().fg(theme::fg())),
    ])
    .style(Style::new().bg(theme::bar_bg()))
}

pub fn centered_rect(width: u16, height: u16, area: Rect) -> Rect {
    let [row] = Layout::vertical([Constraint::Length(height.min(area.height))])
        .flex(Flex::Center)
        .areas(area);
    let [rect] = Layout::horizontal([Constraint::Length(width.min(area.width))])
        .flex(Flex::Center)
        .areas(row);
    rect
}

pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "K", "M", "G", "T"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// Ergebnis einer Tasteneingabe in ein [`LineInput`].
#[derive(Debug, PartialEq, Eq)]
pub enum InputResult {
    Continue,
    Submit,
    Cancel,
}

/// Überschreibt den Inhalt mit Nullen, bevor der Speicher freigegeben wird
/// (Passwörter sollen nicht im freien Heap liegen bleiben).
pub fn wipe_string(s: &mut String) {
    // SAFETY: Null-Bytes sind gültiges UTF-8; danach wird der String geleert.
    let bytes = unsafe { s.as_mut_vec() };
    let len = bytes.capacity();
    bytes.resize(len, 0);
    for b in bytes.iter_mut() {
        // SAFETY: Zeiger auf ein gültiges, exklusiv geliehenes Byte.
        unsafe { std::ptr::write_volatile(b, 0) };
    }
    std::sync::atomic::compiler_fence(std::sync::atomic::Ordering::SeqCst);
    bytes.clear();
}

/// Einzeilige Texteingabe (Umbenennen, Suchen, Speichern unter …).
/// Der Inhalt wird beim Löschen und Freigeben überschrieben.
#[derive(Debug, Default, Clone)]
pub struct LineInput {
    text: String,
    cursor: usize,
}

impl Drop for LineInput {
    fn drop(&mut self) {
        wipe_string(&mut self.text);
    }
}

impl LineInput {
    pub fn new(text: impl Into<String>) -> Self {
        let text = text.into();
        let cursor = text.chars().count();
        Self { text, cursor }
    }

    /// Leere Eingabe für Geheimnisse mit Platz, damit beim Tippen keine Kopien entstehen.
    pub fn secret() -> Self {
        Self {
            text: String::with_capacity(256),
            cursor: 0,
        }
    }

    /// Löscht den Inhalt sicher.
    pub fn wipe(&mut self) {
        wipe_string(&mut self.text);
        self.cursor = 0;
    }

    /// Kopie des Inhalts; der Aufrufer muss sie mit [`wipe_string`] löschen.
    pub fn take_text(&mut self) -> String {
        let out = self.text.clone();
        self.wipe();
        out
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    fn byte_index(&self, char_index: usize) -> usize {
        self.text
            .char_indices()
            .nth(char_index)
            .map_or(self.text.len(), |(i, _)| i)
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> InputResult {
        match key.code {
            KeyCode::Enter => return InputResult::Submit,
            KeyCode::Esc => return InputResult::Cancel,
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                return InputResult::Cancel;
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => self.wipe(),
            KeyCode::Char(c) => {
                if self.text.len() + c.len_utf8() > self.text.capacity() {
                    // Selbst vergrößern, damit der alte Puffer überschrieben wird.
                    let mut bigger = String::with_capacity((self.text.capacity() * 2).max(64));
                    bigger.push_str(&self.text);
                    wipe_string(&mut self.text);
                    self.text = bigger;
                }
                let i = self.byte_index(self.cursor);
                self.text.insert(i, c);
                self.cursor += 1;
            }
            KeyCode::Backspace if self.cursor > 0 => {
                self.cursor -= 1;
                let i = self.byte_index(self.cursor);
                self.text.remove(i);
            }
            KeyCode::Delete if self.cursor < self.text.chars().count() => {
                let i = self.byte_index(self.cursor);
                self.text.remove(i);
            }
            KeyCode::Left => self.cursor = self.cursor.saturating_sub(1),
            KeyCode::Right => self.cursor = (self.cursor + 1).min(self.text.chars().count()),
            KeyCode::Home => self.cursor = 0,
            KeyCode::End => self.cursor = self.text.chars().count(),
            _ => {}
        }
        InputResult::Continue
    }

    /// Zeichnet `label` + Eingabe in eine Zeile und setzt den Terminal-Cursor.
    pub fn render(&self, frame: &mut Frame, area: Rect, label: &str) {
        let line = Line::from(vec![
            Span::styled(label.to_string(), Style::new().fg(theme::accent())),
            Span::styled(self.text.clone(), Style::new().fg(theme::fg())),
        ])
        .style(Style::new().bg(theme::bar_bg()));
        frame.render_widget(line, area);
        let x = area.x + (label.chars().count() + self.cursor) as u16;
        frame.set_cursor_position((x.min(area.right().saturating_sub(1)), area.y));
    }
}

/// Steuerung der Ereignisschleife nach einem Ereignis.
#[derive(Debug, PartialEq, Eq)]
pub enum Flow {
    Continue,
    Quit,
}

/// Gemeinsames Interface aller TUI-Apps.
pub trait App {
    fn draw(&mut self, frame: &mut Frame);
    fn on_key(&mut self, key: KeyEvent, term: &mut DefaultTerminal) -> io::Result<Flow>;
    fn on_mouse(&mut self, _mouse: MouseEvent) -> Flow {
        Flow::Continue
    }
    fn on_paste(&mut self, _text: String) {}
    fn tick(&mut self) {}
    fn tick_rate(&self) -> Duration {
        Duration::from_millis(250)
    }
    /// `true` erzwingt ein komplettes Neuzeichnen, z. B. wenn fremde Ausgaben
    /// (Kernel, systemd) auf die Konsole geschrieben haben könnten.
    fn wants_clear(&mut self) -> bool {
        false
    }
}

fn enter(term_mouse: bool) -> io::Result<DefaultTerminal> {
    let terminal = ratatui::init();
    if term_mouse {
        execute!(stdout(), EnableMouseCapture)?;
    }
    execute!(stdout(), EnableBracketedPaste)?;
    Ok(terminal)
}

fn leave(term_mouse: bool) {
    let _ = execute!(stdout(), DisableBracketedPaste);
    if term_mouse {
        let _ = execute!(stdout(), DisableMouseCapture);
    }
    ratatui::restore();
}

/// Startet die App im Vollbild (Alternate Screen) mit Maus-Unterstützung.
pub fn run<A: App>(app: &mut A) -> io::Result<()> {
    let mut terminal = enter(true)?;
    let result = event_loop(app, &mut terminal);
    leave(true);
    result
}

fn event_loop<A: App>(app: &mut A, terminal: &mut DefaultTerminal) -> io::Result<()> {
    let mut last_tick = Instant::now();
    loop {
        if app.wants_clear() {
            terminal.clear()?;
        }
        terminal.draw(|f| app.draw(f))?;
        let timeout = app.tick_rate().saturating_sub(last_tick.elapsed());
        if event::poll(timeout)? {
            let flow = match event::read()? {
                Event::Key(key) if key.kind != KeyEventKind::Release => {
                    app.on_key(key, terminal)?
                }
                Event::Mouse(mouse) => app.on_mouse(mouse),
                Event::Paste(text) => {
                    app.on_paste(text);
                    Flow::Continue
                }
                _ => Flow::Continue,
            };
            if flow == Flow::Quit {
                return Ok(());
            }
        }
        if last_tick.elapsed() >= app.tick_rate() {
            app.tick();
            last_tick = Instant::now();
        }
    }
}

/// Verlässt kurz den Vollbildmodus, um ein anderes Programm (Editor, Bildbetrachter …)
/// im selben Terminal auszuführen, und kehrt danach zurück.
pub fn suspend<T>(terminal: &mut DefaultTerminal, f: impl FnOnce() -> T) -> io::Result<T> {
    leave(true);
    let out = f();
    *terminal = enter(true)?;
    terminal.clear()?;
    Ok(out)
}

/// Wartet im normalen Terminal auf eine beliebige Taste.
pub fn wait_for_key(prompt: &str) {
    use std::io::Write;
    print!("{prompt}");
    let _ = stdout().flush();
    if crossterm::terminal::enable_raw_mode().is_ok() {
        loop {
            match event::read() {
                Ok(Event::Key(k)) if k.kind == KeyEventKind::Press => break,
                Ok(_) => continue,
                Err(_) => break,
            }
        }
        let _ = crossterm::terminal::disable_raw_mode();
    }
    println!();
}

pub fn is_ctrl(key: &KeyEvent, c: char) -> bool {
    key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char(c)
}

/// Findet ein TUILNUX-Werkzeug: zuerst neben der laufenden Binärdatei
/// (z. B. target/debug während der Entwicklung), sonst über PATH.
pub fn tool_path(name: &str) -> std::path::PathBuf {
    let file = format!("{name}{}", std::env::consts::EXE_SUFFIX);
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join(&file)))
        .filter(|p| p.is_file())
        .unwrap_or_else(|| file.into())
}

/// Bevorzugter Editor: $VISUAL, $EDITOR oder tedit.
pub fn editor_command() -> std::process::Command {
    let configured = std::env::var("VISUAL")
        .ok()
        .or_else(|| std::env::var("EDITOR").ok())
        .filter(|e| !e.trim().is_empty());
    match configured {
        Some(cmd) => {
            let mut parts = cmd.split_whitespace();
            let mut c = std::process::Command::new(parts.next().unwrap_or("tedit"));
            c.args(parts);
            c
        }
        None => std::process::Command::new(tool_path("tedit")),
    }
}

/// Öffnet eine Datei mit dem Standardprogramm des Systems (xdg-open unter Linux).
pub fn open_with_system(path: &std::path::Path) -> io::Result<()> {
    let mut cmd = if cfg!(target_os = "windows") {
        let mut c = std::process::Command::new("cmd");
        c.args(["/C", "start", ""]);
        c
    } else if cfg!(target_os = "macos") {
        std::process::Command::new("open")
    } else {
        std::process::Command::new("xdg-open")
    };
    cmd.arg(path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn line_input_edits_unicode() {
        let mut input = LineInput::new("äb");
        input.handle_key(key(KeyCode::Left));
        input.handle_key(key(KeyCode::Char('ö')));
        assert_eq!(input.text(), "äöb");
        input.handle_key(key(KeyCode::Backspace));
        input.handle_key(key(KeyCode::Backspace));
        assert_eq!(input.text(), "b");
        assert_eq!(input.handle_key(key(KeyCode::Enter)), InputResult::Submit);
    }

    #[test]
    fn sizes_are_human_readable() {
        assert_eq!(human_size(512), "512 B");
        assert_eq!(human_size(2048), "2.0 K");
        assert_eq!(human_size(5 * 1024 * 1024), "5.0 M");
    }

    #[test]
    fn ascii_mode_follows_term_unless_forced() {
        assert!(ascii_for(None, Some("linux")));
        assert!(!ascii_for(None, Some("foot")));
        assert!(!ascii_for(None, None));
        assert!(!ascii_for(Some("0"), Some("linux")));
        assert!(ascii_for(Some("1"), Some("xterm-256color")));
        assert!(ascii_for(Some(""), Some("linux")));
    }
}
