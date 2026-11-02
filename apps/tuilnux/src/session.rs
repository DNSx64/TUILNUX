//! Die TUILNUX-Sitzung: Vollbild-Shell als Desktop, Fenster darüber, Taskleiste unten.

use std::collections::HashMap;
use std::io::{Write, stdout};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use crossterm::event::{
    self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, KeyboardEnhancementFlags, MouseButton,
    MouseEvent, MouseEventKind, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::execute;
use ratatui::layout::Rect;
use tuilnux_config::TitleDoubleClick;
use tuilnux_ipc::{Request, Response, Server, TaskInfo, WINDOW_ENV};
use tuilnux_status::{Bar, BarHit, Monitor, Task};
use tuilnux_term::{Pane, PaneOptions, Wallpaper, default_shell, forward_clipboard, program_name};
use tuilnux_window_manager::{Button, Focus, Hit, Wm};

use crate::config::{Config, Prefix, split_command};
use crate::draw::{self, Overlay, View, bar_area, desktop_area, overlay_rect, task_row_at};
use crate::gui::{GuiEvent, GuiWindows};

pub enum Msg {
    Pane(usize, Option<Vec<u8>>),
    Input(Event),
    Ipc(Request, Sender<Response>),
    Gui(GuiEvent),
    /// Meldung eines Hintergrund-Threads für die Statusleiste.
    Notice(String),
}

struct Win {
    pane: Pane,
    title: Option<String>,
    /// Im Hintergrund gestartet (`tuilnux launch`): hat nur ein Fenster, solange
    /// man seine Ausgabe ansieht (`tab --shell NAME`); Schließen versteckt es.
    job: bool,
}

pub struct Session {
    tx: Sender<Msg>,
    desktop: Pane,
    windows: HashMap<usize, Win>,
    wm: Wm,
    size: Rect,
    env: Vec<(String, String)>,
    shell: Vec<String>,
    prefix: Prefix,
    armed: bool,
    overlay: Option<Overlay>,
    notice: Option<(String, Instant)>,
    monitor: Monitor,
    proc_stats: HashMap<u32, (f32, u64)>,
    proc_at: Option<Instant>,
    wallpaper: Wallpaper,
    mouse: bool,
    last_title_click: Option<(usize, Instant)>,
    ascii: bool,
    settings: Config,
    gui: GuiWindows,
    /// Bildschirmhöhe in Pixeln (vom Compositor) und der für die Statusleiste
    /// freigehaltene Streifen (nur mit Wayland-Compositor).
    output_height: Option<u32>,
    bar_reserve: Option<crate::gui::BarReserve>,
    /// Zähler, damit schnell aufeinanderfolgende Bildschirm-Änderungen nur
    /// einmal zum Anwenden der Einstellungen führen.
    outputs_gen: std::sync::Arc<std::sync::atomic::AtomicU64>,
    quit: bool,
}

/// Startet die Sitzung im aktuellen Terminal und kehrt beim Abmelden zurück.
pub fn run(config: Config) -> Result<()> {
    let prefix = Prefix::parse(&config.prefix)?;
    let (tx, rx) = mpsc::channel();
    let ipc_tx = tx.clone();
    let server = Server::start(move |req, reply| {
        let _ = ipc_tx.send(Msg::Ipc(req, reply));
    })
    .context("IPC-Socket konnte nicht geöffnet werden")?;

    let mut terminal = ratatui::init();
    let _ = execute!(stdout(), EnableBracketedPaste);
    // Kitty-Tastaturprotokoll (foot, kitty, …): ohne es kommen Kombinationen wie
    // Strg+Alt+1 als xterm-„modifyOtherKeys“-Folge an, die crossterm verwirft.
    let enhanced = matches!(
        crossterm::terminal::supports_keyboard_enhancement(),
        Ok(true)
    ) && execute!(
        stdout(),
        PushKeyboardEnhancementFlags(
            KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
                | KeyboardEnhancementFlags::REPORT_ALTERNATE_KEYS
        )
    )
    .is_ok();
    let input = tx.clone();
    std::thread::spawn(move || {
        while let Ok(ev) = event::read() {
            if input.send(Msg::Input(ev)).is_err() {
                break;
            }
        }
    });
    let result = (|| {
        let size = terminal.size()?;
        let size = Rect::new(0, 0, size.width, size.height);
        let gui_tx = tx.clone();
        let mut session = Session::new(tx, size, &server, prefix, &config)?;
        match crate::gui::start(move |ev| gui_tx.send(Msg::Gui(ev)).is_ok()) {
            Ok(reserve) => {
                session.bar_reserve = Some(reserve);
                session.apply_outputs(false);
            }
            Err(e) if std::env::var("TUILNUX_BACKEND").as_deref() == Ok("labwc") => {
                session.notify(format!("Grafische Fenster nicht in der Taskleiste: {e:#}"));
            }
            Err(_) => {}
        }
        session.event_loop(&mut terminal, rx)
    })();
    if enhanced {
        let _ = execute!(stdout(), PopKeyboardEnhancementFlags);
    }
    let _ = execute!(stdout(), DisableMouseCapture, DisableBracketedPaste);
    ratatui::restore();
    drop(server);
    result
}

impl Session {
    fn new(
        tx: Sender<Msg>,
        size: Rect,
        server: &Server,
        prefix: Prefix,
        config: &Config,
    ) -> Result<Self> {
        let area = desktop_area(size);
        let env: Vec<(String, String)> = server.env().into_iter().collect();
        let shell = if config.shell.is_empty() {
            default_shell()
        } else {
            config.shell.clone()
        };
        let mut opts = PaneOptions::new(shell.clone(), area.width, area.height);
        opts.env = env.clone();
        opts.env.push((WINDOW_ENV.into(), "0".into()));
        opts.scrollback = config.terminal.scrollback;
        let out = tx.clone();
        let desktop = Pane::spawn(opts, move |d| out.send(Msg::Pane(0, d)).is_ok())
            .context("Desktop-Shell konnte nicht gestartet werden")?;
        let mut session = Self {
            tx,
            desktop,
            windows: HashMap::new(),
            wm: Wm::new(area),
            size,
            env,
            shell,
            prefix,
            armed: false,
            overlay: None,
            notice: None,
            monitor: Monitor::new(),
            proc_stats: HashMap::new(),
            proc_at: None,
            wallpaper: Wallpaper::default(),
            mouse: false,
            last_title_click: None,
            ascii: false,
            settings: Config::default(),
            gui: GuiWindows::default(),
            output_height: None,
            bar_reserve: None,
            outputs_gen: Default::default(),
            quit: false,
        };
        session.apply_settings(config.clone());
        for entry in &config.autostart {
            let command = split_command(entry);
            if command.is_empty() {
                continue;
            }
            if let Err(e) = session.open_window(command, None, None, None) {
                session.notify(format!("Autostart „{entry}“: {e:#}"));
            }
        }
        session.wm.focus_desktop();
        if session.notice.is_none() {
            session.notify(format!(
                "Willkommen bei TUILNUX – {} ? zeigt die Hilfe",
                session.prefix.label()
            ));
        }
        Ok(session)
    }

    /// Übernimmt Einstellungen (beim Start und nach `tuilnux reload`).
    fn apply_settings(&mut self, settings: Config) {
        let w = &settings.windows;
        let size = (w.width > 0 && w.height > 0).then_some((w.width, w.height));
        self.wm
            .set_defaults(size, w.placement == tuilnux_config::Placement::Center);
        self.ascii = w.ascii || tuilnux_ui::ascii_console();
        tuilnux_ui::theme::set(&settings);
        self.settings = settings;
    }

    /// Liest `tuilnux.toml` und das Hintergrundbild neu ein.
    fn reload(&mut self) -> Result<()> {
        let settings = crate::config::load()?;
        let prefix = Prefix::parse(&settings.prefix)?;
        self.prefix = prefix;
        if !settings.shell.is_empty() {
            self.shell = settings.shell.clone();
        }
        self.apply_settings(settings);
        self.wallpaper = Wallpaper::default();

        Ok(())
    }

    fn notice_time(&self) -> Duration {
        Duration::from_secs(self.settings.bar.notice_seconds.max(1))
    }

    fn event_loop(
        &mut self,
        terminal: &mut ratatui::DefaultTerminal,
        rx: Receiver<Msg>,
    ) -> Result<()> {
        loop {
            let first = match rx.recv_timeout(Duration::from_millis(250)) {
                Ok(msg) => Some(msg),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => break,
            };
            for msg in first.into_iter().chain(rx.try_iter()) {
                self.handle(msg);
            }
            self.reap();
            if self.quit {
                break;
            }
            self.sync();
            self.render(terminal)?;
        }
        Ok(())
    }

    fn handle(&mut self, msg: Msg) {
        match msg {
            Msg::Pane(0, Some(bytes)) => forward_clipboard(&self.desktop.process(&bytes)),
            Msg::Pane(0, None) => self.quit = true,
            Msg::Pane(id, Some(bytes)) => {
                if let Some(win) = self.windows.get_mut(&id) {
                    forward_clipboard(&win.pane.process(&bytes));
                }
            }
            Msg::Pane(id, None) => self.close_window(id, "beendet"),
            Msg::Input(ev) => self.input(ev),
            Msg::Ipc(req, reply) => {
                let response = self.ipc(req);
                let _ = reply.send(response);
            }
            Msg::Gui(GuiEvent::Output { height }) => self.output_height = Some(height),
            Msg::Gui(GuiEvent::OutputsChanged) => self.apply_outputs(true),
            Msg::Notice(text) => self.notify(text),

            Msg::Gui(ev) => {
                let wm = &mut self.wm;
                self.gui.apply(ev, || wm.alloc_id());
            }
        }
    }

    /// Bildschirm-Einstellungen (tsettings → Bildschirme) per wlr-randr anwenden,
    /// beim Start und wenn ein Monitor angeschlossen oder entfernt wird.
    fn apply_outputs(&mut self, changed: bool) {
        use std::sync::atomic::Ordering;
        let displays = tuilnux_config::Settings::load_or_default().displays;
        let generation = self.outputs_gen.fetch_add(1, Ordering::SeqCst) + 1;
        let counter = self.outputs_gen.clone();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(if changed { 800 } else { 200 }));
            if counter.load(Ordering::SeqCst) != generation {
                return;
            }
            let text = match tuilnux_config::apply_outputs(&displays, false) {
                Ok(Some(note)) if changed => note,
                Ok(_) if changed => "Bildschirme geändert".into(),
                Ok(_) => return,
                // Ohne eigene Einstellungen ist ein fehlendes wlr-randr kein Fehler.
                Err(_) if !changed && displays.outputs.is_empty() => return,
                Err(e) => format!("Bildschirme: {e:#}"),
            };
            let _ = tx.send(Msg::Notice(text));
        });
    }

    /// Pane-Größen an Fenster und Bildschirm anpassen, Titel übernehmen.
    fn sync_sizes(&mut self) {
        let area = desktop_area(self.size);
        self.desktop.resize(area.width, area.height);
        for w in self.wm.windows().to_vec() {
            if let Some(win) = self.windows.get_mut(&w.id) {
                let inner = w.inner();
                if inner.width > 0 && inner.height > 0 {
                    win.pane.resize(inner.width, inner.height);
                }
                let title = win
                    .pane
                    .parser
                    .callbacks()
                    .title
                    .clone()
                    .or_else(|| win.title.clone())
                    .unwrap_or_else(|| win.pane.name.clone());
                if title != w.title {
                    self.wm.set_title(w.id, &title);
                }
            }
        }
    }

    /// Beendete Programme einsammeln: Fenster schließen, bei der Desktop-Shell abmelden.
    fn reap(&mut self) {
        if self.desktop.try_wait().is_some() {
            self.quit = true;
        }
        let done: Vec<usize> = self
            .windows
            .iter_mut()
            .filter_map(|(id, w)| w.pane.try_wait().map(|_| *id))
            .collect();
        for id in done {
            self.close_window(id, "beendet");
        }
    }

    /// Pixel unterhalb der letzten Desktop-Zeile, also Statusleiste plus Rest
    /// des Bildschirms. `None`, wenn das Terminal keine Pixelgröße meldet.
    fn bar_margin(&self) -> Option<u32> {
        let ws = crossterm::terminal::window_size().ok()?;
        if ws.rows == 0 || ws.height == 0 {
            return None;
        }
        let cell = u32::from(ws.height) / u32::from(ws.rows);
        let bar = bar_area(self.size);
        let top = u32::from(bar.y) * cell;
        Some(match self.output_height {
            Some(height) if height > top => height - top,
            _ => u32::from(bar.height) * cell,
        })
    }

    /// Hält den Streifen der Statusleiste frei, damit maximierte GUI-Fenster
    /// sie nicht verdecken.
    fn sync_bar_reserve(&self) {
        if let (Some(reserve), Some(height)) = (&self.bar_reserve, self.bar_margin()) {
            reserve.set(height);
        }
    }

    /// Größen, Titel und Mausmodus an den aktuellen Zustand anpassen.
    fn sync(&mut self) {
        self.sync_sizes();
        self.sync_bar_reserve();
        let want = !self.wm.is_empty()
            || (self.settings.windows.gui_in_taskbar && !self.gui.is_empty())
            || self.overlay.is_some()
            || self.focused_pane().wants_mouse()
            || self.desktop.wants_mouse()
            || self.desktop.screen().scrollback() > 0;
        if want != self.mouse {
            self.mouse = want;
            let _ = if want {
                execute!(stdout(), EnableMouseCapture)
            } else {
                execute!(stdout(), DisableMouseCapture)
            };
        }
        let mut bell = std::mem::take(&mut self.desktop.parser.callbacks_mut().bell)
            && self.wm.focus() == Focus::Desktop;
        for (id, win) in &mut self.windows {
            if std::mem::take(&mut win.pane.parser.callbacks_mut().bell)
                && self.wm.focus() == Focus::Window(*id)
            {
                bell = true;
            }
        }
        if bell {
            let _ = stdout().write_all(b"\x07");
        }
        if self
            .notice
            .as_ref()
            .is_some_and(|(_, t)| t.elapsed() > self.notice_time())
        {
            self.notice = None;
        }
    }

    fn render(&mut self, terminal: &mut ratatui::DefaultTerminal) -> Result<()> {
        self.monitor.refresh();
        let area = desktop_area(self.size);
        self.wallpaper.update(area.width, area.height);
        let tasks = if matches!(self.overlay, Some(Overlay::Tasks { .. })) {
            self.task_list()
        } else {
            Vec::new()
        };
        let bar_tasks = self.bar_tasks();
        let message = self.message();
        let prefix = self.prefix.label();
        let view = View {
            wm: &self.wm,
            desktop: self.desktop.screen(),
            windows: self
                .windows
                .iter()
                .map(|(id, w)| (*id, w.pane.screen()))
                .collect(),
            wallpaper: Some(&self.wallpaper),
            bar: self.bar(&bar_tasks, message.as_deref()),
            overlay: self.overlay,
            tasks: &tasks,
            prefix: &prefix,
            ascii: self.ascii,
        };
        terminal.draw(|f| draw::draw(f, view))?;
        Ok(())
    }

    fn bar<'a>(&'a self, tasks: &'a [Task], message: Option<&'a str>) -> Bar<'a> {
        Bar {
            title: self.desktop.title(),
            desktop_focused: self.wm.focus() == Focus::Desktop && self.gui.active().is_none(),
            tasks,
            stats: &self.monitor.stats,
            mouse: self.mouse,
            scrolled: self.desktop.screen().scrollback(),
            message,
            ascii: self.ascii,
            options: &self.settings.bar,
        }
    }

    /// Grafische Fenster, die in Taskleiste und Taskmanager erscheinen.
    fn gui_tasks(&self) -> impl Iterator<Item = &crate::gui::GuiWindow> {
        self.gui
            .iter()
            .filter(|_| self.settings.windows.gui_in_taskbar)
    }

    /// Ist `id` ein grafisches Fenster?
    fn is_gui(&self, id: usize) -> bool {
        self.gui.get(id).is_some()
    }

    /// Fokus innerhalb von TUILNUX, solange kein grafisches Fenster aktiv ist.
    fn tui_focus(&self) -> Option<Focus> {
        self.gui.active().is_none().then(|| self.wm.focus())
    }

    fn bar_tasks(&self) -> Vec<Task> {
        let focus = self.tui_focus();
        let mut tasks: Vec<Task> = self
            .wm
            .by_id()
            .into_iter()
            .map(|w| Task {
                id: w.id,
                title: w.title.clone(),
                focused: focus == Some(Focus::Window(w.id)),
                minimized: w.minimized,
                gui: false,
            })
            .chain(self.gui_tasks().map(|w| Task {
                id: w.id,
                title: w.title().to_string(),
                focused: w.state.activated && !w.state.minimized,
                minimized: w.state.minimized,
                gui: true,
            }))
            .collect();
        tasks.sort_by_key(|t| t.id);
        tasks
    }

    /// Nummern in der Reihenfolge des Taskmanagers (0 = Desktop zuerst).
    fn task_ids(&self) -> Vec<usize> {
        let mut ids: Vec<usize> = self
            .wm
            .by_id()
            .iter()
            .map(|w| w.id)
            .chain(self.gui.iter().map(|w| w.id))
            .collect();
        ids.sort_unstable();
        ids.insert(0, 0);
        ids
    }

    fn message(&self) -> Option<String> {
        if self.armed {
            return Some(
                "MANAGER  Tab 1-9 0  n neu  x zu  m min  f max  ←↑→↓ bewegen  t Tasks  ? Hilfe  Esc"
                    .into(),
            );
        }
        self.notice.as_ref().map(|(m, _)| m.clone())
    }

    fn notify(&mut self, text: impl Into<String>) {
        self.notice = Some((text.into(), Instant::now()));
    }

    fn focused_pane(&self) -> &Pane {
        match self.wm.focus() {
            Focus::Window(id) => self.windows.get(&id).map_or(&self.desktop, |w| &w.pane),
            Focus::Desktop => &self.desktop,
        }
    }

    fn focused_pane_mut(&mut self) -> &mut Pane {
        match self.wm.focus() {
            Focus::Window(id) if self.windows.contains_key(&id) => {
                &mut self.windows.get_mut(&id).unwrap().pane
            }
            _ => &mut self.desktop,
        }
    }

    fn open_window(
        &mut self,
        command: Vec<String>,
        title: Option<String>,
        cwd: Option<PathBuf>,
        size: Option<(u16, u16)>,
    ) -> Result<usize> {
        anyhow::ensure!(!command.is_empty(), "kein Befehl angegeben");
        let name = title.clone().unwrap_or_else(|| program_name(&command));
        let id = self.wm.open(&name, size);
        let inner = self.wm.get(id).map(|w| w.inner()).unwrap_or_default();
        let mut opts = PaneOptions::new(command, inner.width.max(1), inner.height.max(1));
        opts.cwd = cwd;
        opts.scrollback = self.settings.terminal.scrollback;
        opts.env = self.env.clone();
        opts.env.push((WINDOW_ENV.into(), id.to_string()));
        let tx = self.tx.clone();
        match Pane::spawn(opts, move |d| tx.send(Msg::Pane(id, d)).is_ok()) {
            Ok(pane) => {
                self.windows.insert(
                    id,
                    Win {
                        pane,
                        title,
                        job: false,
                    },
                );
                Ok(id)
            }
            Err(e) => {
                self.wm.close(id);
                Err(e)
            }
        }
    }

    fn open_shell(&mut self) {
        if let Err(e) = self.open_window(self.shell.clone(), None, None, None) {
            self.notify(format!("Terminal: {e:#}"));
        }
    }

    /// Startet ein Programm im Hintergrund: eigenes Terminal, aber kein Fenster.
    fn launch(&mut self, command: Vec<String>, cwd: Option<PathBuf>) -> Result<usize> {
        anyhow::ensure!(!command.is_empty(), "kein Befehl angegeben");
        let id = self.wm.alloc_id();
        let area = desktop_area(self.size);
        let (cols, rows) = (
            (area.width * 2 / 3).saturating_sub(2).max(20),
            (area.height * 2 / 3).saturating_sub(2).max(5),
        );
        let mut opts = PaneOptions::new(command, cols, rows);
        opts.cwd = cwd;
        opts.scrollback = self.settings.terminal.scrollback;
        opts.env = self.env.clone();
        opts.env.push((WINDOW_ENV.into(), id.to_string()));
        let tx = self.tx.clone();
        let pane = Pane::spawn(opts, move |d| tx.send(Msg::Pane(id, d)).is_ok())?;
        self.windows.insert(
            id,
            Win {
                pane,
                title: None,
                job: true,
            },
        );
        Ok(id)
    }

    /// Schließen per Maus oder Taste: Hintergrundprogramme laufen weiter, nur
    /// ihr Fenster verschwindet.
    fn dismiss_window(&mut self, id: usize) {
        match self.windows.get(&id) {
            Some(win) if win.job => {
                let name = win.pane.name.clone();
                if self.wm.close(id) {
                    self.notify(format!(
                        "{name} läuft im Hintergrund weiter (tab --shell {name})"
                    ));
                }
            }
            _ => self.close_window(id, "geschlossen"),
        }
    }

    fn close_window(&mut self, id: usize, why: &str) {
        let title = self.wm.get(id).map(|w| w.title.clone());
        // Drop beendet den Prozess, falls er noch läuft.
        let removed = self.windows.remove(&id);
        let shown = self.wm.close(id);
        if let Some(win) = removed.filter(|w| w.job) {
            self.notify(format!("{} (Hintergrund, {id}) {why}", win.pane.name));
        } else if shown && let Some(title) = title {
            self.notify(format!("Fenster {id} ({title}) {why}"));
        }
        if let Some(Overlay::Tasks { selected }) = &mut self.overlay {
            *selected = (*selected).min(self.wm.len());
        }
    }

    fn focus_id(&mut self, id: usize) -> bool {
        if self.is_gui(id) {
            return self.gui.activate(id);
        }
        let ok = if id == 0 {
            self.wm.focus_desktop();
            true
        } else if self.wm.get(id).is_none()
            && let Some(win) = self.windows.get_mut(&id)
        {
            // Hintergrundprogramm: jetzt ein Fenster für seine Ausgabe öffnen.
            let name = win.title.clone().unwrap_or_else(|| win.pane.name.clone());
            let (cols, rows) = win.pane.size();
            self.wm.open_with_id(id, &name, Some((cols, rows)));
            true
        } else {
            self.wm.focus_window(id)
        };
        if ok {
            self.gui.raise_desktop();
        }
        ok
    }

    fn minimize_id(&mut self, id: usize) {
        if !self.gui.minimize(id) {
            self.wm.minimize(id);
        }
    }

    fn maximize_id(&mut self, id: usize) {
        if !self.gui.toggle_maximize(id) {
            self.wm.toggle_maximize(id);
        }
    }

    fn close_id(&mut self, id: usize) -> bool {
        if self.gui.close(id) {
            return true;
        }
        if self.windows.contains_key(&id) {
            self.close_window(id, "geschlossen");
            return true;
        }
        false
    }

    /// Alle Tasks (0 = Desktop) mit CPU- und Speicherverbrauch.
    fn task_list(&mut self) -> Vec<TaskInfo> {
        if self
            .proc_at
            .is_none_or(|t| t.elapsed() >= Duration::from_secs(1))
        {
            let pids: Vec<u32> = std::iter::once(self.desktop.pid())
                .chain(self.windows.values().map(|w| w.pane.pid()))
                .flatten()
                .collect();
            self.proc_stats = self.monitor.processes(&pids);
            self.proc_at = Some(Instant::now());
        }
        let stats = |pid: Option<u32>| {
            pid.and_then(|p| self.proc_stats.get(&p).copied())
                .unwrap_or_default()
        };
        let (cpu, memory) = stats(self.desktop.pid());
        let focus = self.tui_focus();
        let mut out = vec![TaskInfo {
            id: 0,
            title: self.desktop.title().to_string(),
            command: self.desktop.command.join(" "),
            pid: self.desktop.pid(),
            focused: focus == Some(Focus::Desktop),
            minimized: false,
            cpu,
            memory,
            gui: false,
            background: false,
        }];
        for w in self.wm.by_id() {
            let Some(win) = self.windows.get(&w.id) else {
                continue;
            };
            let (cpu, memory) = stats(win.pane.pid());
            out.push(TaskInfo {
                id: w.id,
                title: w.title.clone(),
                command: win.pane.command.join(" "),
                pid: win.pane.pid(),
                focused: focus == Some(Focus::Window(w.id)),
                minimized: w.minimized,
                cpu,
                memory,
                gui: false,
                background: win.job,
            });
        }
        for (id, win) in &self.windows {
            if !win.job || self.wm.get(*id).is_some() {
                continue;
            }
            let (cpu, memory) = stats(win.pane.pid());
            out.push(TaskInfo {
                id: *id,
                title: win.pane.name.clone(),
                command: win.pane.command.join(" "),
                pid: win.pane.pid(),
                focused: false,
                minimized: true,
                cpu,
                memory,
                gui: false,
                background: true,
            });
        }
        for w in self.gui.iter() {
            out.push(TaskInfo {
                id: w.id,
                title: w.title().to_string(),
                command: w.state.app_id.clone(),
                pid: None,
                focused: w.state.activated && !w.state.minimized,
                minimized: w.state.minimized,
                cpu: 0.0,
                memory: 0,
                gui: true,
                background: false,
            });
        }
        out[1..].sort_by_key(|t| t.id);
        out
    }

    fn ipc(&mut self, req: Request) -> Response {
        let missing = |id: usize| Response::error(format!("kein Fenster mit Nummer {id}"));
        match req {
            Request::Open {
                command,
                title,
                cwd,
                size,
            } => match self.open_window(command, title, cwd.map(PathBuf::from), size) {
                Ok(id) => Response::Opened { id },
                Err(e) => Response::error(format!("{e:#}")),
            },
            Request::Launch { command, cwd } => {
                match self.launch(command, cwd.map(PathBuf::from)) {
                    Ok(id) => Response::Opened { id },
                    Err(e) => Response::error(format!("{e:#}")),
                }
            }
            Request::List => Response::Tasks {
                tasks: self.task_list(),
            },
            Request::Focus { id } => {
                if self.focus_id(id) {
                    Response::Ok
                } else {
                    missing(id)
                }
            }
            Request::Close { id: 0 } => Response::error(
                "Der Desktop lässt sich nicht schließen – abmelden mit `tuilnux logout`",
            ),
            Request::Close { id } => {
                if self.close_id(id) {
                    Response::Ok
                } else {
                    missing(id)
                }
            }
            Request::Minimize { id } | Request::Maximize { id }
                if self.wm.get(id).is_none() && !self.is_gui(id) =>
            {
                missing(id)
            }
            Request::Minimize { id } => {
                self.minimize_id(id);
                Response::Ok
            }
            Request::Maximize { id } => {
                self.maximize_id(id);
                Response::Ok
            }
            Request::Notify { text } => {
                self.notify(text.replace(['\n', '\r'], " "));
                Response::Ok
            }
            Request::Logout => {
                self.quit = true;
                Response::Ok
            }
            Request::Reload => match self.reload() {
                Ok(()) => {
                    self.notify("Einstellungen neu geladen");
                    Response::Ok
                }
                Err(e) => Response::error(format!("{e:#}")),
            },
        }
    }

    fn input(&mut self, ev: Event) {
        match ev {
            Event::Key(key) if key.kind != KeyEventKind::Release => self.key(key),
            Event::Paste(text) if self.overlay.is_none() => self.focused_pane_mut().paste(&text),
            Event::Mouse(m) => self.mouse(m),
            Event::Resize(w, h) => {
                self.size = Rect::new(0, 0, w, h);
                self.wm.set_area(desktop_area(self.size));
            }
            _ => {}
        }
    }

    fn key(&mut self, key: KeyEvent) {
        if let Some(overlay) = self.overlay {
            return self.overlay_key(overlay, key);
        }
        if self.prefix.matches(&key) {
            if self.armed {
                // Zweimal gedrückt: die Taste selbst an das Programm schicken.
                self.armed = false;
                self.focused_pane_mut().key(key);
            } else {
                self.armed = true;
            }
            return;
        }
        if self.armed {
            self.armed = self.command(key);
            return;
        }
        if self.settings.windows.quick_switch
            && let Some(id) = quick_switch(&key)
        {
            self.quick_switch(id);
            return;
        }
        self.focused_pane_mut().key(key);
    }

    /// Strg+Alt+Ziffer: Fenster wiederherstellen und fokussieren, 0 = Desktop.
    fn quick_switch(&mut self, id: usize) {
        if !self.focus_id(id) {
            self.notify(format!("Kein Fenster {id}"));
        }
    }

    /// Befehl im Manager-Modus. Gibt zurück, ob der Modus aktiv bleibt.
    fn command(&mut self, key: KeyEvent) -> bool {
        let focused = self.wm.focused();
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        let arrow = match key.code {
            KeyCode::Left => Some((-1, 0)),
            KeyCode::Right => Some((1, 0)),
            KeyCode::Up => Some((0, -1)),
            KeyCode::Down => Some((0, 1)),
            _ => None,
        };
        if let Some((dx, dy)) = arrow {
            match focused {
                Some(id) if shift => {
                    self.wm.resize_by(id, dx * 2, dy);
                }
                Some(id) => {
                    self.wm.move_by(id, dx * 2, dy);
                }
                None => self.notify("Der Desktop ist immer Vollbild"),
            }
            // Pfeile lassen den Modus aktiv, damit man mehrmals schieben kann.
            return true;
        }
        match key.code {
            KeyCode::Tab | KeyCode::BackTab => self.wm.focus_next(),
            KeyCode::Char(c @ '1'..='9') => {
                let id = c as usize - '0' as usize;
                if !self.focus_id(id) {
                    self.notify(format!("Kein Fenster {id}"));
                }
            }
            KeyCode::Char('0' | 'd') => self.wm.focus_desktop(),
            KeyCode::Enter | KeyCode::Char('n' | 'c') => self.open_shell(),
            KeyCode::Char('t') => {
                self.overlay = Some(Overlay::Tasks {
                    selected: self.selected_index(),
                })
            }
            KeyCode::Char('s') => {
                let tool = tuilnux_ui::tool_path("tsettings").display().to_string();
                if let Err(e) =
                    self.open_window(vec![tool], Some("Einstellungen".into()), None, None)
                {
                    self.notify(format!("Einstellungen: {e:#}"));
                }
            }
            KeyCode::Char('?' | 'h') => self.overlay = Some(Overlay::Help),
            KeyCode::Char('q') => self.overlay = Some(Overlay::Logout),
            KeyCode::Char('x') => match focused {
                Some(id) => self.dismiss_window(id),
                None => self.notify("Der Desktop bleibt offen – abmelden mit q"),
            },
            KeyCode::Char('m') => {
                if let Some(id) = focused {
                    self.wm.minimize(id);
                }
            }
            KeyCode::Char('f') => {
                if let Some(id) = focused {
                    self.wm.toggle_maximize(id);
                }
            }
            KeyCode::Esc => {}
            _ => self.notify(format!(
                "Unbekannter Befehl – {} dann ? zeigt die Hilfe",
                self.prefix.label()
            )),
        }
        false
    }

    /// Zeile der Taskliste, die zum aktuellen Fokus gehört.
    fn selected_index(&self) -> usize {
        let id = match (self.gui.active(), self.wm.focus()) {
            (Some(id), _) | (None, Focus::Window(id)) => id,
            (None, Focus::Desktop) => 0,
        };
        self.task_ids().iter().position(|&i| i == id).unwrap_or(0)
    }

    /// Fensternummer zu einer Zeile der Taskliste (0 = Desktop).
    fn task_id(&self, row: usize) -> Option<usize> {
        self.task_ids().get(row).copied()
    }

    fn overlay_key(&mut self, overlay: Overlay, key: KeyEvent) {
        match overlay {
            Overlay::Logout => {
                if matches!(
                    key.code,
                    KeyCode::Char('j' | 'J' | 'y' | 'Y') | KeyCode::Enter
                ) {
                    self.quit = true;
                }
                self.overlay = None;
            }
            Overlay::Help => self.overlay = None,
            Overlay::Tasks { selected } => {
                let count = self.task_ids().len();
                let id = self.task_id(selected);
                match key.code {
                    KeyCode::Up | KeyCode::Char('k') => {
                        self.overlay = Some(Overlay::Tasks {
                            selected: selected.saturating_sub(1),
                        })
                    }
                    KeyCode::Down | KeyCode::Char('j') | KeyCode::Tab => {
                        self.overlay = Some(Overlay::Tasks {
                            selected: (selected + 1).min(count - 1),
                        })
                    }
                    KeyCode::Enter => {
                        if let Some(id) = id {
                            self.focus_id(id);
                        }
                        self.overlay = None;
                    }
                    KeyCode::Char(c @ '0'..='9') => {
                        if self.focus_id(c as usize - '0' as usize) {
                            self.overlay = None;
                        }
                    }
                    KeyCode::Char('m') => {
                        if let Some(id) = id.filter(|&id| id > 0) {
                            self.minimize_id(id);
                        }
                    }
                    KeyCode::Char('f') => {
                        if let Some(id) = id.filter(|&id| id > 0) {
                            self.maximize_id(id);
                        }
                    }
                    KeyCode::Char('x') | KeyCode::Delete => match id {
                        Some(0) | None => self.notify("Der Desktop bleibt offen – abmelden mit q"),
                        Some(id) => {
                            self.close_id(id);
                        }
                    },
                    KeyCode::Char('n') => {
                        self.overlay = None;
                        self.open_shell();
                    }
                    KeyCode::Char('q') => self.overlay = Some(Overlay::Logout),
                    _ if self.prefix.matches(&key) => self.overlay = None,
                    KeyCode::Esc | KeyCode::Char('t') => self.overlay = None,
                    _ => {}
                }
            }
        }
    }

    fn mouse(&mut self, m: MouseEvent) {
        let (x, y) = (m.column, m.row);
        if let Some(overlay) = self.overlay {
            return self.overlay_mouse(overlay, m);
        }
        let bar = bar_area(self.size);
        if bar.height > 0 && y == bar.y {
            if m.kind == MouseEventKind::Down(MouseButton::Left) {
                let tasks = self.bar_tasks();
                let message = self.message();
                let hit = self.bar(&tasks, message.as_deref()).hit(x);
                match hit {
                    Some(BarHit::Menu) => {
                        self.overlay = Some(Overlay::Tasks {
                            selected: self.selected_index(),
                        })
                    }
                    Some(BarHit::Desktop) => {
                        self.wm.focus_desktop();
                        self.gui.raise_desktop();
                    }
                    Some(BarHit::Task(id)) if self.is_gui(id) => {
                        if self.gui.was_active(id) {
                            self.gui.minimize(id);
                        } else {
                            self.gui.activate(id);
                        }
                    }
                    Some(BarHit::Task(id)) => {
                        let visible = self.wm.get(id).is_some_and(|w| !w.minimized);
                        if visible && self.wm.focused() == Some(id) && self.gui.active().is_none() {
                            self.wm.minimize(id);
                        } else {
                            self.focus_id(id);
                        }
                    }
                    None => {}
                }
            }
            return;
        }
        match m.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                let hit = self.wm.mouse_down(x, y);
                match hit {
                    Hit::Button(id, Button::Close) => self.dismiss_window(id),
                    Hit::Title(id) => {
                        let now = Instant::now();
                        let double = Duration::from_millis(self.settings.mouse.double_click_ms);
                        if self
                            .last_title_click
                            .is_some_and(|(last, t)| last == id && t.elapsed() < double)
                        {
                            self.wm.mouse_up();
                            match self.settings.windows.double_click {
                                TitleDoubleClick::Maximize => self.wm.toggle_maximize(id),
                                TitleDoubleClick::Minimize => self.wm.minimize(id),
                                TitleDoubleClick::Nothing => {}
                            }
                            self.last_title_click = None;
                        } else {
                            self.last_title_click = Some((id, now));
                        }
                    }
                    Hit::Content { id, col, row } => {
                        if let Some(win) = self.windows.get_mut(&id) {
                            win.pane.mouse(m, col, row);
                        }
                    }
                    Hit::Desktop => self.desktop.mouse(m, x, y),
                    _ => {}
                }
            }
            MouseEventKind::Drag(MouseButton::Left) if self.wm.dragging() => {
                self.wm.mouse_drag(x, y);
            }
            MouseEventKind::Up(MouseButton::Left) if self.wm.dragging() => self.wm.mouse_up(),
            MouseEventKind::Down(_) | MouseEventKind::Up(_) | MouseEventKind::Drag(_) => {
                // Klicks und Ziehen gehören dem fokussierten Programm (z. B. Textauswahl).
                match self.wm.focus() {
                    Focus::Window(id) => {
                        if let (Some(w), Some(win)) = (self.wm.get(id), self.windows.get_mut(&id)) {
                            let inner = w.inner();
                            let col = x.saturating_sub(inner.x).min(inner.width.saturating_sub(1));
                            let row = y
                                .saturating_sub(inner.y)
                                .min(inner.height.saturating_sub(1));
                            win.pane.mouse(m, col, row);
                        }
                    }
                    Focus::Desktop => self.desktop.mouse(m, x, y),
                }
            }
            _ => match self.wm.hit_test(x, y) {
                Hit::Content { id, col, row } => {
                    if let Some(win) = self.windows.get_mut(&id) {
                        win.pane.mouse(m, col, row);
                    }
                }
                Hit::Desktop => self.desktop.mouse(m, x, y),
                _ => {}
            },
        }
    }

    fn overlay_mouse(&mut self, overlay: Overlay, m: MouseEvent) {
        let count = self.task_ids().len();
        let rect = overlay_rect(desktop_area(self.size), overlay, count);
        let inside = m.column >= rect.x
            && m.column < rect.right()
            && m.row >= rect.y
            && m.row < rect.bottom();
        match (overlay, m.kind) {
            (_, MouseEventKind::Down(MouseButton::Left)) if !inside => self.overlay = None,
            (Overlay::Tasks { selected }, MouseEventKind::Down(MouseButton::Left)) => {
                if let Some(row) = task_row_at(rect, m.row, count) {
                    if row == selected {
                        if let Some(id) = self.task_id(row) {
                            self.focus_id(id);
                        }
                        self.overlay = None;
                    } else {
                        self.overlay = Some(Overlay::Tasks { selected: row });
                    }
                }
            }
            (Overlay::Tasks { selected }, MouseEventKind::ScrollUp) => {
                self.overlay = Some(Overlay::Tasks {
                    selected: selected.saturating_sub(1),
                })
            }
            (Overlay::Tasks { selected }, MouseEventKind::ScrollDown) => {
                self.overlay = Some(Overlay::Tasks {
                    selected: (selected + 1).min(count - 1),
                })
            }
            (Overlay::Help, MouseEventKind::Down(_)) => self.overlay = None,
            _ => {}
        }
    }
}

/// Fensternummer bei Strg+Alt+Ziffer. Viele Terminals verschlucken dabei das Strg
/// und senden nur Alt+Ziffer (ESC 1), deshalb zählt Alt allein ebenfalls.
fn quick_switch(key: &KeyEvent) -> Option<usize> {
    let mods = key.modifiers;
    if !mods.contains(KeyModifiers::ALT)
        || mods.intersects(KeyModifiers::SHIFT | KeyModifiers::SUPER)
    {
        return None;
    }
    match key.code {
        KeyCode::Char(c @ '0'..='9') => Some(c as usize - '0' as usize),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shell() -> Vec<String> {
        if cfg!(windows) {
            vec!["cmd.exe".into()]
        } else {
            vec!["/bin/sh".into()]
        }
    }

    fn session() -> (Session, Receiver<Msg>, Server) {
        let (tx, rx) = mpsc::channel();
        let ipc = tx.clone();
        let server = Server::start(move |req, reply| {
            let _ = ipc.send(Msg::Ipc(req, reply));
        })
        .unwrap();
        let config = Config {
            shell: shell(),
            ..Config::default()
        };
        let prefix = Prefix::parse(&config.prefix).unwrap();
        let s = Session::new(tx, Rect::new(0, 0, 100, 30), &server, prefix, &config).unwrap();
        (s, rx, server)
    }

    fn key(code: KeyCode, modifiers: KeyModifiers) -> Event {
        Event::Key(KeyEvent::new(code, modifiers))
    }

    fn click(kind: MouseEventKind, column: u16, row: u16) -> Event {
        Event::Mouse(MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        })
    }

    #[test]
    fn ipc_controls_windows() {
        let (mut s, _rx, _server) = session();
        let open = Request::Open {
            command: shell(),
            title: Some("Dateien".into()),
            cwd: None,
            size: Some((40, 10)),
        };
        assert_eq!(s.ipc(open), Response::Opened { id: 1 });
        assert_eq!(s.wm.focused(), Some(1));
        assert_eq!(s.windows[&1].pane.size(), (40, 10));
        let Response::Tasks { tasks } = s.ipc(Request::List) else {
            panic!()
        };
        assert_eq!(tasks.len(), 2);
        assert_eq!((tasks[1].id, tasks[1].title.as_str()), (1, "Dateien"));
        assert!(tasks[1].pid.is_some() && tasks[1].focused);

        assert_eq!(s.ipc(Request::Focus { id: 0 }), Response::Ok);
        assert_eq!(s.wm.focus(), Focus::Desktop);
        assert_eq!(s.ipc(Request::Minimize { id: 1 }), Response::Ok);
        assert!(s.wm.get(1).unwrap().minimized);
        assert!(matches!(
            s.ipc(Request::Close { id: 0 }),
            Response::Error { .. }
        ));
        assert!(matches!(
            s.ipc(Request::Focus { id: 9 }),
            Response::Error { .. }
        ));
        assert!(matches!(
            s.ipc(Request::Open {
                command: vec!["gibt-es-nicht-tuilnux".into()],
                title: None,
                cwd: None,
                size: None
            }),
            Response::Error { .. }
        ));
        assert_eq!(
            s.wm.len(),
            1,
            "fehlgeschlagenes Öffnen hinterlässt kein Fenster"
        );
        assert_eq!(s.ipc(Request::Close { id: 1 }), Response::Ok);
        assert!(s.wm.is_empty() && s.windows.is_empty());
        assert!(
            s.message()
                .unwrap()
                .contains("Fenster 1 (Dateien) geschlossen")
        );
        assert_eq!(
            s.ipc(Request::Notify {
                text: "Build\nfertig".into()
            }),
            Response::Ok
        );
        assert_eq!(s.message().as_deref(), Some("Build fertig"));
        s.ipc(Request::Logout);
        assert!(s.quit);
    }

    #[test]
    fn real_client_round_trip() {
        let (mut s, rx, server) = session();
        let (addr, token) = (server.address().to_string(), server.token().to_string());
        let client = std::thread::spawn(move || {
            tuilnux_ipc::request(
                &addr,
                &token,
                Request::Open {
                    command: shell(),
                    title: None,
                    cwd: None,
                    size: None,
                },
            )
        });
        let deadline = Instant::now() + Duration::from_secs(10);
        while !client.is_finished() && Instant::now() < deadline {
            if let Ok(msg) = rx.recv_timeout(Duration::from_millis(50)) {
                s.handle(msg);
            }
        }
        assert_eq!(client.join().unwrap().unwrap(), Response::Opened { id: 1 });
        assert_eq!(s.wm.len(), 1);
    }

    #[test]
    fn prefix_keys_and_overlays() {
        let (mut s, _rx, _server) = session();
        let prefix = key(KeyCode::Char(' '), KeyModifiers::CONTROL);
        s.input(prefix.clone());
        assert!(s.armed && s.message().unwrap().starts_with("MANAGER"));
        s.input(key(KeyCode::Enter, KeyModifiers::NONE));
        assert!(!s.armed);
        assert_eq!(s.wm.focused(), Some(1), "Enter öffnet ein Terminal-Fenster");

        let before = s.wm.get(1).unwrap().rect;
        s.input(prefix.clone());
        s.input(key(KeyCode::Right, KeyModifiers::NONE));
        s.input(key(KeyCode::Right, KeyModifiers::NONE));
        assert!(s.armed, "Pfeile lassen den Manager-Modus offen");
        s.input(key(KeyCode::Down, KeyModifiers::SHIFT));
        s.input(key(KeyCode::Esc, KeyModifiers::NONE));
        let after = s.wm.get(1).unwrap().rect;
        assert_eq!((after.x, after.height), (before.x + 4, before.height + 1));

        s.input(prefix.clone());
        s.input(key(KeyCode::Char('t'), KeyModifiers::NONE));
        assert_eq!(s.overlay, Some(Overlay::Tasks { selected: 1 }));
        s.input(key(KeyCode::Up, KeyModifiers::NONE));
        s.input(key(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!((s.overlay, s.wm.focus()), (None, Focus::Desktop));

        s.input(prefix.clone());
        s.input(key(KeyCode::Char('1'), KeyModifiers::NONE));
        assert_eq!(s.wm.focused(), Some(1));
        s.input(prefix.clone());
        s.input(key(KeyCode::Char('x'), KeyModifiers::NONE));
        assert!(s.wm.is_empty());

        s.input(prefix.clone());
        s.input(key(KeyCode::Char('q'), KeyModifiers::NONE));
        assert_eq!(s.overlay, Some(Overlay::Logout));
        s.input(key(KeyCode::Char('n'), KeyModifiers::NONE));
        assert!(!s.quit && s.overlay.is_none());
        s.input(prefix.clone());
        s.input(key(KeyCode::Char('q'), KeyModifiers::NONE));
        s.input(key(KeyCode::Char('j'), KeyModifiers::NONE));
        assert!(s.quit);
    }

    #[test]
    fn mouse_drags_windows_and_uses_the_bar() {
        let (mut s, _rx, _server) = session();
        s.open_window(shell(), Some("a".into()), None, Some((40, 10)))
            .unwrap();
        let r = s.wm.get(1).unwrap().rect;
        s.input(click(MouseEventKind::Down(MouseButton::Left), r.x + 5, r.y));
        s.input(click(
            MouseEventKind::Drag(MouseButton::Left),
            r.x + 25,
            r.y + 6,
        ));
        s.input(click(
            MouseEventKind::Up(MouseButton::Left),
            r.x + 25,
            r.y + 6,
        ));
        let moved = s.wm.get(1).unwrap().rect;
        assert_eq!((moved.x, moved.y), (r.x + 20, r.y + 6));
        s.sync_sizes();
        assert_eq!(s.windows[&1].pane.size(), (40, 10));

        // Klick auf den Desktop, dann auf den Task in der Leiste.
        s.input(click(MouseEventKind::Down(MouseButton::Left), 0, 0));
        assert_eq!(s.wm.focus(), Focus::Desktop);
        let tasks = s.bar_tasks();
        let bar = s.bar(&tasks, None);
        let x = (0..100)
            .find(|&x| bar.hit(x) == Some(BarHit::Task(1)))
            .unwrap();
        s.input(click(MouseEventKind::Down(MouseButton::Left), x, 29));
        assert_eq!(s.wm.focused(), Some(1));
        // Erneuter Klick minimiert.
        s.input(click(MouseEventKind::Down(MouseButton::Left), x, 29));
        assert!(s.wm.get(1).unwrap().minimized);
        // Schließen-Knopf.
        s.input(click(MouseEventKind::Down(MouseButton::Left), x, 29));
        let r = s.wm.get(1).unwrap().rect;
        s.input(click(
            MouseEventKind::Down(MouseButton::Left),
            Button::Close.x(r),
            r.y,
        ));
        assert!(s.wm.is_empty());
    }

    #[test]
    fn mouse_resizes_windows_and_panes() {
        let (mut s, _rx, _server) = session();
        s.open_window(shell(), Some("a".into()), None, Some((40, 10)))
            .unwrap();
        let r = s.wm.get(1).unwrap().rect;
        // Rechte Kante ziehen.
        s.input(click(
            MouseEventKind::Down(MouseButton::Left),
            r.right() - 1,
            r.y + 3,
        ));
        s.input(click(
            MouseEventKind::Drag(MouseButton::Left),
            r.right() + 9,
            r.y + 3,
        ));
        s.input(click(
            MouseEventKind::Up(MouseButton::Left),
            r.right() + 9,
            r.y + 3,
        ));
        // Ecke unten rechts ziehen.
        let r2 = s.wm.get(1).unwrap().rect;
        s.input(click(
            MouseEventKind::Down(MouseButton::Left),
            r2.right() - 1,
            r2.bottom() - 1,
        ));
        s.input(click(
            MouseEventKind::Drag(MouseButton::Left),
            r2.right() - 6,
            r2.bottom() + 3,
        ));
        s.input(click(
            MouseEventKind::Up(MouseButton::Left),
            r2.right() - 6,
            r2.bottom() + 3,
        ));
        let r3 = s.wm.get(1).unwrap().rect;
        assert_eq!((r3.width, r3.height), (r.width + 5, r.height + 4));
        s.sync_sizes();
        assert_eq!(s.windows[&1].pane.size(), (45, 14));
    }

    #[test]
    fn ctrl_alt_digit_restores_windows() {
        let (mut s, _rx, _server) = session();
        s.open_window(shell(), Some("a".into()), None, Some((40, 10)))
            .unwrap();
        s.open_window(shell(), Some("b".into()), None, Some((40, 10)))
            .unwrap();
        s.wm.minimize(1);
        s.wm.minimize(2);
        assert_eq!(s.wm.focus(), Focus::Desktop);
        let ctrl_alt = KeyModifiers::CONTROL | KeyModifiers::ALT;
        s.input(key(KeyCode::Char('1'), ctrl_alt));
        assert_eq!(s.wm.focused(), Some(1));
        assert!(!s.wm.get(1).unwrap().minimized);
        // Terminals, die nur ESC 2 (Alt+2) schicken, funktionieren auch.
        s.input(key(KeyCode::Char('2'), KeyModifiers::ALT));
        assert_eq!(s.wm.focused(), Some(2));
        s.input(key(KeyCode::Char('0'), ctrl_alt));
        assert_eq!(s.wm.focus(), Focus::Desktop);
        s.input(key(KeyCode::Char('7'), ctrl_alt));
        assert!(s.message().unwrap().contains("Kein Fenster 7"));
        assert_eq!(
            quick_switch(&KeyEvent::new(KeyCode::Char('1'), KeyModifiers::NONE)),
            None
        );
    }

    #[test]
    fn closed_programs_close_their_window() {
        let (mut s, rx, _server) = session();
        let quick = if cfg!(windows) {
            vec!["cmd.exe".into(), "/c".into(), "echo".into(), "hi".into()]
        } else {
            vec!["/bin/sh".into(), "-c".into(), "echo hi".into()]
        };
        s.open_window(quick, None, None, None).unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while !s.wm.is_empty() && Instant::now() < deadline {
            if let Ok(msg) = rx.recv_timeout(Duration::from_millis(50)) {
                s.handle(msg);
            }
            s.reap();
        }
        assert!(s.wm.is_empty());
        assert!(s.message().unwrap().contains("beendet"));
        assert!(!s.quit);
    }

    fn gui(n: u64, app_id: &str, minimized: bool, activated: bool) -> Msg {
        Msg::Gui(GuiEvent::Update(crate::gui::GuiState {
            handle: crate::gui::Handle::fake(n),
            title: format!("{app_id} Fenster"),
            app_id: app_id.into(),
            minimized,
            maximized: false,
            activated,
        }))
    }

    #[test]
    fn gui_windows_join_taskbar_and_tab() {
        let (mut s, _rx, _server) = session();
        s.open_window(shell(), Some("Dateien".into()), None, None)
            .unwrap();
        s.handle(gui(1, crate::gui::DESKTOP_APP_ID, false, false));
        s.handle(gui(2, "firefox", false, true));
        let tasks = s.bar_tasks();
        assert_eq!(tasks.len(), 2);
        assert!(tasks[1].gui && tasks[1].focused && tasks[1].id == 2);
        assert!(
            !tasks[0].focused,
            "TUILNUX-Fenster hat keinen Fokus, wenn Firefox aktiv ist"
        );
        assert!(!s.bar(&tasks, None).desktop_focused);

        let list = s.task_list();
        assert_eq!(list.iter().map(|t| t.id).collect::<Vec<_>>(), [0, 1, 2]);
        assert!(list[2].gui && list[2].command == "firefox");
        assert_eq!(crate::cli::find_task(&list, "firefox").unwrap().id, 2);
        assert_eq!(s.task_ids(), [0, 1, 2]);
        assert_eq!(s.selected_index(), 2);

        assert_eq!(s.ipc(Request::Minimize { id: 2 }), Response::Ok);
        assert!(s.gui.get(2).unwrap().state.minimized);
        assert_eq!(s.gui.active(), None);
        assert_eq!(s.ipc(Request::Focus { id: 2 }), Response::Ok);
        assert_eq!(s.gui.active(), Some(2));
        // Strg+Alt+1 holt das TUILNUX-Fenster zurück und gibt den Fokus ab.
        s.input(key(
            KeyCode::Char('1'),
            KeyModifiers::CONTROL | KeyModifiers::ALT,
        ));
        assert_eq!(s.gui.active(), None);
        assert_eq!(s.wm.focus(), Focus::Window(1));

        s.handle(Msg::Gui(GuiEvent::Closed(crate::gui::Handle::fake(2))));
        assert_eq!(s.bar_tasks().len(), 1);
        assert!(matches!(
            s.ipc(Request::Focus { id: 2 }),
            Response::Error { .. }
        ));

        s.settings.windows.gui_in_taskbar = false;
        s.handle(gui(3, "gimp", false, false));
        assert_eq!(s.bar_tasks().len(), 1, "ausgeblendet in der Taskleiste");
        assert_eq!(s.task_list().len(), 3, "aber für tab erreichbar");
    }

    #[test]
    fn settings_shape_the_session() {
        let (mut s, _rx, _server) = session();
        let mut settings = s.settings.clone();
        settings.windows.width = 40;
        settings.windows.height = 10;
        settings.windows.placement = tuilnux_config::Placement::Center;
        settings.windows.quick_switch = false;
        settings.bar.notice_seconds = 9;
        s.apply_settings(settings);
        assert_eq!(s.notice_time(), Duration::from_secs(9));
        let id = s.open_window(shell(), None, None, None).unwrap();
        let w = s.wm.get(id).unwrap();
        assert_eq!((w.inner().width, w.inner().height), (40, 10));
        let area = desktop_area(s.size);
        assert_eq!(w.rect.x, area.x + (area.width - 42) / 2);
        s.wm.focus_desktop();
        s.input(key(
            KeyCode::Char('1'),
            KeyModifiers::CONTROL | KeyModifiers::ALT,
        ));
        assert_eq!(
            s.wm.focus(),
            Focus::Desktop,
            "Schnellwechsel ist abgeschaltet"
        );
    }
}
