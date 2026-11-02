//! `tuilnux search`: installierte Programme suchen und starten (Super+F).
//!
//! In einer Sitzung startet die Suche Programme über die IPC: grafische im
//! Hintergrund, Terminal-Programme als TUILNUX-Fenster. Ohne Sitzung startet
//! sie grafische Programme selbst und führt Terminal-Programme im eigenen
//! Terminal aus.

use std::collections::HashMap;
use std::io;
use std::process::{Command, Stdio};

use anyhow::{Result, bail};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{List, ListItem, ListState, Paragraph};
use ratatui::{DefaultTerminal, Frame};
use tuilnux_ipc::{Request, Response};
use tuilnux_ui::{App, Flow, InputResult, LineInput, theme, tr, trf};

use crate::apps::{self, AppEntry, Source};

/// app_id des Such-Fensters unter labwc (Fensterregel: mittig, immer oben).
pub const APP_ID: &str = "tuilnux-search";
const POPUP_SIZE: (u16, u16) = (76, 22);

const USAGE: &str = "\
tuilnux search [TEXT]           Programmsuche im aktuellen Terminal
tuilnux search --popup [TEXT]   in einem eigenen Terminal-Fenster (Super+F unter labwc)
tuilnux search --list [TEXT]    Treffer nur ausgeben
";

pub fn run(args: &[String]) -> Result<()> {
    let (mut popup, mut list, mut words) = (false, false, Vec::new());
    for arg in args {
        match arg.as_str() {
            "--popup" => popup = true,
            "--list" => list = true,
            "-h" | "--help" => {
                print!("{USAGE}");
                return Ok(());
            }
            a if a.starts_with("--") => bail!("search: unbekannte Option „{a}“"),
            a => words.push(a.to_string()),
        }
    }
    let query = words.join(" ");
    if popup {
        return open_popup(&query);
    }
    let settings = theme::reload();
    let locales = apps::locales(&tuilnux_ui::i18n::current());
    let apps = apps::installed(&locales, shell(&settings));
    let history = apps::load_history();
    if list {
        use std::io::Write;
        let mut out = io::stdout().lock();
        for i in apps::rank(&apps, &query, &history) {
            let e = &apps[i];
            // Abgebrochene Pipe (`| head`) ist kein Fehler.
            if writeln!(out, "{}\t{}\t{}", e.name, kind(e), e.exec.join(" ")).is_err() {
                break;
            }
        }
        return Ok(());
    }
    let mut search = Search::new(apps, history, &query);
    tuilnux_ui::run(&mut search)?;
    match search.outcome.take() {
        Some(Outcome::Exec(command)) => exec(&command),
        _ => Ok(()),
    }
}

fn shell(settings: &tuilnux_config::Settings) -> Vec<String> {
    if !settings.shell.is_empty() {
        return settings.shell.clone();
    }
    vec![std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into())]
}

fn kind(e: &AppEntry) -> &'static str {
    match e.source {
        Source::Builtin => "tuilnux",
        Source::Path => "befehl",
        Source::Desktop if e.terminal => "terminal",
        Source::Desktop => "grafisch",
    }
}

/// Öffnet die Suche in einem eigenen kleinen Terminal-Fenster (für Tastenkürzel
/// des Compositors, die außerhalb des Desktop-Terminals laufen).
fn open_popup(query: &str) -> Result<()> {
    let settings = tuilnux_config::Settings::load_or_default();
    let me = std::env::current_exe().unwrap_or_else(|_| "tuilnux".into());
    let mut inner = vec![me.display().to_string(), "search".into()];
    if !query.is_empty() {
        inner.push(query.into());
    }
    let (cols, rows) = POPUP_SIZE;
    let font = format!(
        "font={}:size={}",
        settings.terminal.font, settings.terminal.font_size
    );
    let preferred = std::env::var("TUILNUX_TERMINAL").unwrap_or_default();
    let terminal = [preferred.as_str(), "foot", "kitty", "alacritty"]
        .into_iter()
        .find(|t| !t.is_empty() && which(t))
        .ok_or_else(|| anyhow::anyhow!("kein Terminal (foot, kitty, alacritty) gefunden"))?;
    let mut cmd = Command::new(terminal);
    match terminal {
        "foot" => {
            cmd.arg(format!("--app-id={APP_ID}"))
                .arg(format!("--window-size-chars={cols}x{rows}"))
                .args(["-o", "csd.preferred=none", "-o", &font]);
        }
        "kitty" => {
            cmd.args(["--class", APP_ID])
                .args(["-o", &format!("initial_window_width={cols}c")])
                .args(["-o", &format!("initial_window_height={rows}c")])
                .args(["-o", "remember_window_size=no"]);
        }
        "alacritty" => {
            cmd.args(["--class", APP_ID])
                .args(["-o", &format!("window.dimensions.columns={cols}")])
                .args(["-o", &format!("window.dimensions.lines={rows}")])
                .arg("-e");
        }
        _ => {
            cmd.arg("-e");
        }
    }
    cmd.args(&inner);
    run_replacing(cmd)
}

fn which(program: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|paths| std::env::split_paths(&paths).any(|dir| dir.join(program).is_file()))
}

#[cfg(unix)]
fn run_replacing(mut cmd: Command) -> Result<()> {
    use std::os::unix::process::CommandExt;
    let err = cmd.exec();
    bail!("{}: {err}", cmd.get_program().to_string_lossy())
}

#[cfg(not(unix))]
fn run_replacing(mut cmd: Command) -> Result<()> {
    let status = cmd.status()?;
    std::process::exit(status.code().unwrap_or(1));
}

/// Terminal-Programm ohne Sitzung: ersetzt die Suche im selben Terminal.
fn exec(command: &[String]) -> Result<()> {
    let mut cmd = Command::new(&command[0]);
    cmd.args(&command[1..]);
    run_replacing(cmd)
}

/// Grafisches Programm ohne Sitzung: unabhängig vom Terminal starten.
fn spawn_detached(command: &[String]) -> io::Result<()> {
    let mut cmd = Command::new(&command[0]);
    cmd.args(&command[1..])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if let Some(home) = std::env::var_os("HOME") {
        cmd.current_dir(home);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    cmd.spawn().map(drop)
}

enum Outcome {
    Started,
    Exec(Vec<String>),
}

struct Search {
    apps: Vec<AppEntry>,
    history: HashMap<String, u32>,
    input: LineInput,
    hits: Vec<usize>,
    list: ListState,
    list_area: Rect,
    message: Option<String>,
    outcome: Option<Outcome>,
}

impl Search {
    fn new(apps: Vec<AppEntry>, history: HashMap<String, u32>, query: &str) -> Self {
        let mut s = Self {
            apps,
            history,
            input: LineInput::new(query),
            hits: Vec::new(),
            list: ListState::default(),
            list_area: Rect::default(),
            message: None,
            outcome: None,
        };
        s.refresh();
        s
    }

    fn refresh(&mut self) {
        self.hits = apps::rank(&self.apps, self.input.text(), &self.history);
        self.list.select((!self.hits.is_empty()).then_some(0));
        *self.list.offset_mut() = 0;
    }

    fn step(&mut self, delta: isize) {
        if self.hits.is_empty() {
            return;
        }
        let n = self.hits.len() as isize;
        let cur = self.list.selected().unwrap_or(0) as isize;
        self.list.select(Some((cur + delta).rem_euclid(n) as usize));
    }

    fn selected(&self) -> Option<&AppEntry> {
        self.list
            .selected()
            .and_then(|i| self.hits.get(i))
            .map(|&i| &self.apps[i])
    }

    /// Startet den markierten Eintrag; `true`, wenn die Suche sich schließen soll.
    fn start(&mut self) -> bool {
        let Some(entry) = self.selected().cloned() else {
            return false;
        };
        match start(&entry) {
            Ok(outcome) => {
                apps::record_use(&entry.id);
                self.outcome = Some(outcome);
                true
            }
            Err(e) => {
                self.message = Some(format!("{}: {e:#}", entry.name));
                false
            }
        }
    }
}

/// Grafisches Programm? `.desktop` mit `Terminal=false` oder ein Befehl, den die
/// Shell-Einstellungen als grafisch kennen (Firefox ohne `.desktop`-Datei …).
fn is_gui(entry: &AppEntry) -> bool {
    match entry.source {
        Source::Builtin => false,
        Source::Desktop => !entry.terminal,
        Source::Path => {
            let settings = tuilnux_config::Settings::load_or_default();
            tuilnux_config::gui_programs(&settings.bash)
                .iter()
                .any(|p| p == entry.program())
        }
    }
}

fn start(entry: &AppEntry) -> Result<Outcome> {
    let gui = is_gui(entry);
    let cwd = std::env::var("HOME").ok();
    let request = if gui {
        Request::Launch {
            command: entry.exec.clone(),
            cwd,
        }
    } else {
        Request::Open {
            command: entry.exec.clone(),
            title: Some(entry.name.clone()),
            cwd,
            size: None,
        }
    };
    match tuilnux_ipc::request_env(request) {
        Ok(Response::Error { message }) => bail!("{message}"),
        Ok(_) => Ok(Outcome::Started),
        Err(e) if e.kind() == io::ErrorKind::NotFound && gui => {
            spawn_detached(&entry.exec)?;
            Ok(Outcome::Started)
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Outcome::Exec(entry.exec.clone())),
        Err(e) => bail!("{}", trf("Sitzung nicht erreichbar: {}", &[&e])),
    }
}

impl App for Search {
    fn draw(&mut self, frame: &mut Frame) {
        let area = frame.area();
        let block = tuilnux_ui::frame_block("Programme suchen", true);
        let inner = block.inner(area);
        frame.render_widget(block, area);
        let [input, gap, list, status, keys] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .areas(inner);
        let _ = gap;
        self.input.render(frame, input, " > ");
        self.list_area = list;

        let width = list.width as usize;
        let items: Vec<ListItem> = self
            .hits
            .iter()
            .map(|&i| {
                let e = &self.apps[i];
                let tag = match e.source {
                    Source::Builtin => tr("TUILNUX"),
                    Source::Path => tr("Befehl"),
                    Source::Desktop if e.terminal => tr("Terminal"),
                    Source::Desktop => "",
                };
                let name = format!(" {}", e.name);
                let summary = e.summary();
                let used = name.chars().count() + tag.chars().count() + 2;
                let room = width.saturating_sub(used + 3);
                let mut detail: String = summary.chars().take(room).collect();
                if summary.chars().count() > room && room > 0 {
                    detail.pop();
                    detail.push('…');
                }
                let pad = width.saturating_sub(used + detail.chars().count() + 2);
                ListItem::new(Line::from(vec![
                    Span::styled(
                        name,
                        Style::new().fg(theme::fg()).add_modifier(Modifier::BOLD),
                    ),
                    Span::raw("  "),
                    Span::styled(detail, Style::new().fg(theme::dim())),
                    Span::raw(" ".repeat(pad)),
                    Span::styled(tag, Style::new().fg(theme::accent2())),
                    Span::raw(" "),
                ]))
            })
            .collect();
        if items.is_empty() {
            frame.render_widget(
                Paragraph::new(format!(" {}", tr("Keine Programme gefunden")))
                    .style(Style::new().fg(theme::dim())),
                list,
            );
        } else {
            let widget = List::new(items).highlight_style(
                Style::new()
                    .bg(theme::sel_bg())
                    .fg(theme::accent())
                    .add_modifier(Modifier::BOLD),
            );
            frame.render_stateful_widget(widget, list, &mut self.list);
        }

        let left = match &self.message {
            Some(m) => m.clone(),
            None => self
                .selected()
                .map(|e| format!(" {}", e.exec.join(" ")))
                .unwrap_or_default(),
        };
        let right = trf("{} Treffer ", &[&self.hits.len()]);
        frame.render_widget(tuilnux_ui::status_line(left, right, status.width), status);
        frame.render_widget(
            tuilnux_ui::keybar(&[
                ("Enter", "Starten"),
                ("↑↓", "Auswahl"),
                ("Esc", "Schließen"),
            ]),
            keys,
        );
    }

    fn on_key(&mut self, key: KeyEvent, _term: &mut DefaultTerminal) -> io::Result<Flow> {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Up => self.step(-1),
            KeyCode::Down | KeyCode::Tab => self.step(1),
            KeyCode::BackTab => self.step(-1),
            KeyCode::Char('p') if ctrl => self.step(-1),
            KeyCode::Char('n') if ctrl => self.step(1),
            KeyCode::PageUp => self.step(-(self.list_area.height.max(2) as isize - 1)),
            KeyCode::PageDown => self.step(self.list_area.height.max(2) as isize - 1),
            _ => {
                let before = self.input.text().to_string();
                match self.input.handle_key(key) {
                    InputResult::Submit => {
                        if self.start() {
                            return Ok(Flow::Quit);
                        }
                    }
                    InputResult::Cancel => return Ok(Flow::Quit),
                    InputResult::Continue => {
                        if self.input.text() != before {
                            self.message = None;
                            self.refresh();
                        }
                    }
                }
            }
        }
        Ok(Flow::Continue)
    }

    fn on_mouse(&mut self, m: MouseEvent) -> Flow {
        match m.kind {
            MouseEventKind::ScrollDown => self.step(1),
            MouseEventKind::ScrollUp => self.step(-1),
            MouseEventKind::Down(MouseButton::Left) => {
                let a = self.list_area;
                if m.column >= a.x && m.column < a.right() && m.row >= a.y && m.row < a.bottom() {
                    let row = (m.row - a.y) as usize + self.list.offset();
                    if row < self.hits.len() {
                        self.list.select(Some(row));
                        if self.start() {
                            return Flow::Quit;
                        }
                    }
                }
            }
            _ => {}
        }
        Flow::Continue
    }

    fn on_paste(&mut self, text: String) {
        for c in text.chars().filter(|c| !c.is_control()) {
            self.input
                .handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        }
        self.refresh();
    }

    fn tick_rate(&self) -> std::time::Duration {
        std::time::Duration::from_secs(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str) -> AppEntry {
        AppEntry {
            id: name.into(),
            name: name.into(),
            generic: String::new(),
            comment: String::new(),
            keywords: Vec::new(),
            exec: vec![name.to_lowercase()],
            terminal: false,
            source: Source::Desktop,
        }
    }

    #[test]
    fn typing_filters_and_arrows_select() {
        let apps = vec![entry("Firefox"), entry("Files"), entry("Gimp")];
        let mut s = Search::new(apps, HashMap::new(), "");
        assert_eq!(s.hits.len(), 3);
        s.on_paste("fi".into());
        assert_eq!(s.hits.len(), 2);
        assert_eq!(s.selected().unwrap().name, "Files");
        s.step(1);
        assert_eq!(s.selected().unwrap().name, "Firefox");
        s.step(1);
        assert_eq!(s.selected().unwrap().name, "Files");
        s.on_paste("ref".into());
        assert_eq!(s.hits.len(), 1);
        s.on_paste("xyz".into());
        assert!(s.selected().is_none());
        assert!(!s.start());
    }
}
