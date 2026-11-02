//! tuilnux-setup – grafische Ersteinrichtung von TUILNUX.
//!
//! Wird von `install.sh` bei der ersten Installation gestartet: Sprache,
//! Tastatur, Bildschirm (Auflösung und Bildrate aus der EDID) und Komponenten
//! wählen, dann installiert der Assistent und zeigt den Fortschritt live an.
//! Ohne Quellbaum (`install.sh`) richtet er nur Sprache, Tastatur und Bildschirm ein.

mod drm;
mod run;

use std::collections::VecDeque;
use std::io;
use std::path::PathBuf;
use std::process::Command;
use std::sync::mpsc::TryRecvError;
use std::time::Duration;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use image::DynamicImage;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Gauge, Paragraph, Wrap};
use ratatui::{DefaultTerminal, Frame};
use tuilnux_ui::{App, Flow, frame_block, i18n, keybar, theme, tr, trf};

use crate::drm::Output;
use crate::run::{Msg, Plan, Runner};

const LOGO: &[u8] = include_bytes!("../../../assets/tuilnux-logo.png");

/// Bezeichnung, Konsolen-Keymap (`localectl set-keymap`), XKB-Layout (labwc).
const KEYMAPS: &[(&str, &str, &str)] = &[
    ("Deutsch", "de-latin1", "de"),
    ("Deutsch (Schweiz)", "de_CH-latin1", "ch"),
    ("Deutsch (Österreich)", "de-latin1-nodeadkeys", "at"),
    ("English (US)", "us", "us"),
    ("English (UK)", "uk", "gb"),
    ("Français", "fr-latin1", "fr"),
    ("Español", "es", "es"),
    ("Italiano", "it", "it"),
    ("Nederlands", "nl", "nl"),
    ("Polski", "pl", "pl"),
];

const BACKENDS: &[(&str, &str)] = &[
    ("auto", "automatisch"),
    ("labwc", "labwc (Fenster für grafische Programme)"),
    ("cage", "cage (nur das Terminal)"),
    ("tty", "tty (Linux-Konsole)"),
];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Page {
    Welcome,
    Keyboard,
    Display,
    Components,
    Summary,
    Install,
    Done,
}

impl Page {
    fn title(self) -> &'static str {
        match self {
            Page::Welcome => "Willkommen",
            Page::Keyboard => "Tastatur",
            Page::Display => "Bildschirm",
            Page::Components => "Komponenten",
            Page::Summary => "Zusammenfassung",
            Page::Install => "Installation",
            Page::Done => "Fertig",
        }
    }
}

struct Component {
    label: &'static str,
    help: &'static str,
    on: bool,
}

const GRAPHICAL: usize = 0;
const GREETD: usize = 1;
const ENABLE: usize = 2;
const BASHRC: usize = 3;
const FONT: usize = 4;
const NVIDIA: usize = 5;
const TESTS: usize = 6;

struct Setup {
    page: Page,
    root: Option<PathBuf>,
    logo: Option<DynamicImage>,
    logo_cache: Option<((u16, u16), Vec<Line<'static>>)>,
    languages: Vec<(String, String)>,
    lang: usize,
    keymap: usize,
    outputs: Vec<Output>,
    output: usize,
    /// Gewählter Modus je Bildschirm (0 = automatisch).
    modes: Vec<usize>,
    components: Vec<Component>,
    nvidia_found: bool,
    comp: usize,
    backend: usize,
    message: Option<String>,
    runner: Option<Runner>,
    log: VecDeque<String>,
    steps: usize,
    total: usize,
    compiled: usize,
    current_step: String,
    result: Option<Result<(), String>>,
}

impl Setup {
    fn new(root: Option<PathBuf>) -> Self {
        let languages = i18n::available();
        let current = i18n::resolve("auto");
        let lang = languages
            .iter()
            .position(|(c, _)| *c == current)
            .unwrap_or(0);
        let outputs = drm::detect();
        let nvidia_found = has_nvidia();
        let components = vec![
            Component {
                label: "Grafische Programme (labwc, Xwayland, foot)",
                help: "Firefox, Discord & Co. laufen als verschiebbare Fenster neben dem Terminal-Desktop. Aus: nur die Linux-Konsole.",
                on: true,
            },
            Component {
                label: "Anmeldebildschirm tlogin (greetd)",
                help: "Sichere Anmeldung im TUILNUX-Design mit Sicherheitsschlüssel, Handy-Code und QR-Kopplung.",
                on: true,
            },
            Component {
                label: "Anmeldung beim Booten starten",
                help: "Ersetzt den Text-Login auf TTY1. Ein vorhandener Anmeldemanager wird deaktiviert (Deinstallation stellt ihn wieder her).",
                on: true,
            },
            Component {
                label: "~/.bashrc anpassen (Begrüßung, Aliase, Bilder)",
                help: "Begrüßung per asciia, Aliase wie open/files/top/edit und bild/einfuegen für Bilder im Terminal.",
                on: true,
            },
            Component {
                label: "Konsolenschrift Terminus",
                help: "Echte Rahmenlinien auf der Linux-Konsole (ter-v16n, ab 2560 px ter-v32n).",
                on: true,
            },
            Component {
                label: "NVIDIA-Treiber für Wayland",
                help: "nvidia-open-dkms, egl-wayland und nvidia_drm.modeset=1.",
                on: nvidia_found,
            },
            Component {
                label: "Tests beim Bauen ausführen",
                help: "Prüft TUILNUX vor der Installation (dauert einige Minuten länger).",
                on: false,
            },
        ];
        let keymap = current_keymap()
            .and_then(|k| KEYMAPS.iter().position(|(_, c, _)| *c == k))
            .unwrap_or_else(|| keymap_for(&languages[lang].0));
        let mut setup = Self {
            page: Page::Welcome,
            root,
            logo: image::load_from_memory(LOGO).ok(),
            logo_cache: None,
            modes: vec![0; outputs.len()],
            languages,
            lang,
            keymap,
            outputs,
            output: 0,
            components,
            nvidia_found,
            comp: 0,
            backend: 0,
            message: None,
            runner: None,
            log: VecDeque::new(),
            steps: 0,
            total: 1,
            compiled: 0,
            current_step: String::new(),
            result: None,
        };
        setup.apply_language();
        setup
    }

    fn lang_code(&self) -> &str {
        &self.languages[self.lang].0
    }

    fn apply_language(&mut self) {
        i18n::init(&self.languages[self.lang].0.clone());
    }

    fn pages(&self) -> Vec<Page> {
        let mut p = vec![Page::Welcome, Page::Keyboard, Page::Display];
        if self.root.is_some() {
            p.push(Page::Components);
        }
        p.extend([Page::Summary, Page::Install, Page::Done]);
        p
    }

    fn step(&mut self, delta: isize) {
        let pages = self.pages();
        let i = pages.iter().position(|p| *p == self.page).unwrap_or(0) as isize;
        let next = (i + delta).clamp(0, pages.len() as isize - 1) as usize;
        self.page = pages[next];
        self.message = None;
    }

    fn plan(&self) -> Plan {
        let on = |i: usize| self.components[i].on;
        let full = self.root.is_some();
        Plan {
            graphical: on(GRAPHICAL),
            greetd: on(GREETD),
            enable: on(ENABLE) && on(GREETD),
            bashrc: on(BASHRC),
            font: on(FONT),
            nvidia: full && on(NVIDIA) && on(GRAPHICAL),
            tests: on(TESTS),
            backend: BACKENDS[self.backend].0,
            locale: i18n::locale_for(self.lang_code()),
            keymap: KEYMAPS[self.keymap].1.to_string(),
        }
    }

    fn mode_choice(&self, i: usize) -> Option<&drm::Mode> {
        match self.modes.get(i).copied().unwrap_or(0) {
            0 => None,
            n => self.outputs[i].modes.get(n - 1),
        }
    }

    fn display_summary(&self) -> String {
        if self.outputs.is_empty() {
            return tr("automatisch beim Start").into();
        }
        self.outputs
            .iter()
            .enumerate()
            .map(|(i, o)| {
                let mode = match self.mode_choice(i) {
                    Some(m) => m.label(),
                    None => trf(
                        "automatisch ({})",
                        &[&o.best().map_or_else(|| "?".into(), |m| m.label())],
                    ),
                };
                format!("{}: {mode}", o.name)
            })
            .collect::<Vec<_>>()
            .join(" · ")
    }

    fn start_install(&mut self, term: &mut DefaultTerminal) -> io::Result<()> {
        let ok = tuilnux_ui::suspend(term, || {
            println!();
            println!(
                "{}",
                tr("Für die Installation wird dein Passwort benötigt (sudo).")
            );
            Command::new("sudo")
                .arg("-v")
                .status()
                .is_ok_and(|s| s.success())
        })?;
        if !ok {
            self.message =
                Some(tr("sudo-Anmeldung fehlgeschlagen – bitte erneut versuchen.").into());
            return Ok(());
        }
        let plan = self.plan();
        let commands = match &self.root {
            Some(root) => {
                self.total = run::estimate_steps(root, &plan);
                vec![run::install_command(root, &plan)]
            }
            None => {
                self.total = 2;
                vec![run::system_command(&plan)]
            }
        };
        self.log.clear();
        self.steps = 0;
        self.compiled = 0;
        self.current_step.clear();
        self.result = None;
        self.runner = Some(run::start(commands));
        self.page = Page::Install;
        self.message = None;
        Ok(())
    }

    fn poll(&mut self) {
        let Some(runner) = &self.runner else {
            return;
        };
        let mut finished = None;
        loop {
            match runner.rx.try_recv() {
                Ok(Msg::Line(line)) => {
                    let step = run::is_step(&line);
                    let line = run::display_line(line);
                    if step {
                        self.steps += 1;
                        self.compiled = 0;
                        self.current_step = line.trim_start_matches("==>").trim().to_string();
                    } else if line.trim_start().starts_with("Compiling ") {
                        self.compiled += 1;
                    }
                    self.log.push_back(line);
                    if self.log.len() > 500 {
                        self.log.pop_front();
                    }
                }
                Ok(Msg::Finished(r)) => {
                    finished = Some(r);
                    break;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    finished = Some(Err(tr("Installationsprozess beendet").into()));
                    break;
                }
            }
        }
        if let Some(result) = finished {
            self.runner = None;
            let result = result.and_then(|()| self.write_user_settings());
            self.log.push_back(match &result {
                Ok(()) => format!("==> {}", tr("Fertig.")),
                Err(e) => format!("==> {}", trf("Fehler: {}", &[e])),
            });
            self.result = Some(result);
            self.page = Page::Done;
        }
    }

    /// Sprache, Tastatur und Bildschirme in `~/.config/tuilnux/tuilnux.toml` übernehmen.
    fn write_user_settings(&mut self) -> Result<(), String> {
        let mut s = tuilnux_config::Settings::load_or_default();
        s.language = self.lang_code().to_string();
        s.keyboard.layout = KEYMAPS[self.keymap].2.to_string();
        s.displays.max_refresh = true;
        for (i, o) in self.outputs.iter().enumerate() {
            let key = self.mode_choice(i).map(drm::Mode::key).unwrap_or_default();
            if !key.is_empty() || s.displays.output(&o.name).is_some() {
                s.displays.output_mut(&o.name).mode = key;
            }
        }
        match tuilnux_config::apply(&s, None) {
            Ok(applied) => {
                for f in applied.written {
                    self.log.push_back(format!("    {}", f.display()));
                }
                self.log
                    .push_back(format!("==> {}", tr("Benutzereinstellungen gespeichert")));
                theme::set(&s);
                Ok(())
            }
            Err(e) => Err(format!("{e:#}")),
        }
    }

    fn progress(&self) -> f64 {
        if self.result.is_some() {
            return 1.0;
        }
        // Innerhalb eines Bauschritts wandert der Balken mit den kompilierten Crates mit.
        let partial = (self.compiled as f64 / 160.0).min(0.9);
        ((self.steps as f64 + partial) / (self.total as f64 + 1.0)).min(0.99)
    }

    fn logo_lines(&mut self, cols: u16, rows: u16) -> Vec<Line<'static>> {
        let Some(img) = &self.logo else {
            return Vec::new();
        };
        let size = tuilnux_media::fit_cells(img.width(), img.height(), cols, rows);
        if let Some((cached, lines)) = &self.logo_cache
            && *cached == size
        {
            return lines.clone();
        }
        // Die Konsolenschrift (Terminus) kennt kein ▀ und nur 8 Hintergrundfarben –
        // dort wird das Logo zweifarbig aus Leerzeichen gezeichnet.
        let console = std::env::var("TERM").is_ok_and(|t| t == "linux");
        let bright = |t: (u8, u8, u8), b: (u8, u8, u8)| {
            let max = |c: (u8, u8, u8)| u16::from(c.0.max(c.1).max(c.2));
            max(t) + max(b) >= 256
        };
        let lines: Vec<Line<'static>> = tuilnux_media::halfblock_cells(img, size.0, size.1)
            .into_iter()
            .map(|row| {
                Line::from(
                    row.into_iter()
                        .map(|(t, b)| {
                            if console {
                                let bg = if bright(t, b) {
                                    Color::Cyan
                                } else {
                                    Color::Reset
                                };
                                Span::styled(" ", Style::new().bg(bg))
                            } else {
                                Span::styled(
                                    "▀",
                                    Style::new()
                                        .fg(Color::Rgb(t.0, t.1, t.2))
                                        .bg(Color::Rgb(b.0, b.1, b.2)),
                                )
                            }
                        })
                        .collect::<Vec<_>>(),
                )
            })
            .collect();
        self.logo_cache = Some((size, lines.clone()));
        lines
    }
}

fn has_nvidia() -> bool {
    let Ok(dir) = std::fs::read_dir("/sys/bus/pci/devices") else {
        return false;
    };
    dir.flatten().any(|d| {
        let read = |f: &str| std::fs::read_to_string(d.path().join(f)).unwrap_or_default();
        read("vendor").trim() == "0x10de" && read("class").trim().starts_with("0x03")
    })
}

fn current_keymap() -> Option<String> {
    std::fs::read_to_string("/etc/vconsole.conf")
        .ok()?
        .lines()
        .find_map(|l| {
            l.trim()
                .strip_prefix("KEYMAP=")
                .map(|v| v.trim_matches('"').to_string())
        })
}

fn keymap_for(lang: &str) -> usize {
    let xkb = match lang {
        "en" => "us",
        "de" => "de",
        other => other,
    };
    KEYMAPS.iter().position(|(_, _, x)| *x == xkb).unwrap_or(0)
}

/// Liste mit Markierung; scrollt so, dass die Auswahl sichtbar bleibt.
fn list_lines(items: &[String], selected: usize, height: u16, focused: bool) -> Vec<Line<'static>> {
    let height = height.max(1) as usize;
    let start = selected.saturating_sub(height.saturating_sub(1));
    items
        .iter()
        .enumerate()
        .skip(start)
        .take(height)
        .map(|(i, item)| {
            if i == selected {
                let style = if focused {
                    Style::new()
                        .fg(theme::fg())
                        .bg(theme::sel_bg())
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::new().fg(theme::accent())
                };
                Line::from(Span::styled(format!(" ▸ {item} "), style))
            } else {
                Line::from(Span::styled(
                    format!("   {item}"),
                    Style::new().fg(theme::fg()),
                ))
            }
        })
        .collect()
}

fn text(s: impl Into<String>) -> Line<'static> {
    Line::from(Span::styled(s.into(), Style::new().fg(theme::fg())))
}

fn dim(s: impl Into<String>) -> Line<'static> {
    Line::from(Span::styled(s.into(), Style::new().fg(theme::dim())))
}

fn heading(s: impl Into<String>) -> Line<'static> {
    Line::from(Span::styled(
        s.into(),
        Style::new()
            .fg(theme::accent())
            .add_modifier(Modifier::BOLD),
    ))
}

impl App for Setup {
    fn draw(&mut self, frame: &mut Frame) {
        let area = frame.area();
        let [header, body, message, keys] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Min(5),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .areas(area);

        let pages = self.pages();
        let index = pages.iter().position(|p| *p == self.page).unwrap_or(0);
        let crumbs: Vec<Span> = pages
            .iter()
            .enumerate()
            .flat_map(|(i, p)| {
                let style = if i == index {
                    Style::new()
                        .fg(theme::accent())
                        .add_modifier(Modifier::BOLD)
                } else if i < index {
                    Style::new().fg(theme::fg())
                } else {
                    Style::new().fg(theme::dim())
                };
                let sep = if i == 0 { "" } else { " › " };
                [
                    Span::styled(sep, Style::new().fg(theme::dim())),
                    Span::styled(tr(p.title()).to_string(), style),
                ]
            })
            .collect();
        let mut head = vec![Span::styled(
            " TUILNUX ",
            Style::new()
                .fg(Color::Black)
                .bg(theme::accent())
                .add_modifier(Modifier::BOLD),
        )];
        head.push(Span::raw("  "));
        head.extend(crumbs);
        frame.render_widget(
            Paragraph::new(Line::from(head)).style(Style::new().bg(theme::bar_bg())),
            header,
        );

        let title = trf("Schritt {} von {}", &[&(index + 1), &pages.len()]);
        let block = frame_block(self.page.title(), true).title_bottom(
            Line::from(Span::styled(
                format!(" {title} "),
                Style::new().fg(theme::dim()),
            ))
            .alignment(Alignment::Right),
        );
        let inner = block.inner(body);
        frame.render_widget(block, body);
        match self.page {
            Page::Welcome => self.draw_welcome(frame, inner),
            Page::Keyboard => self.draw_keyboard(frame, inner),
            Page::Display => self.draw_display(frame, inner),
            Page::Components => self.draw_components(frame, inner),
            Page::Summary => self.draw_summary(frame, inner),
            Page::Install | Page::Done => self.draw_install(frame, inner),
        }

        if let Some(msg) = &self.message {
            frame.render_widget(
                Paragraph::new(Span::styled(
                    format!(" {msg}"),
                    Style::new().fg(theme::warn()),
                )),
                message,
            );
        }
        let bar: &[(&str, &str)] = match self.page {
            Page::Welcome => &[("↑↓", "Sprache"), ("Enter", "Weiter"), ("^Q", "Beenden")],
            Page::Keyboard => &[
                ("↑↓", "Auswahl"),
                ("Enter", "Weiter"),
                ("Esc", "Zurück"),
                ("^Q", "Beenden"),
            ],
            Page::Display => &[
                ("←→", "Bildschirm"),
                ("↑↓", "Modus"),
                ("Enter", "Weiter"),
                ("Esc", "Zurück"),
            ],
            Page::Components => &[
                ("↑↓", "Auswahl"),
                ("Leertaste", "An/Aus"),
                ("Enter", "Weiter"),
                ("Esc", "Zurück"),
            ],
            Page::Summary => &[
                ("Enter", "Installieren"),
                ("Esc", "Zurück"),
                ("^Q", "Beenden"),
            ],
            Page::Install => &[("", "Bitte warten …")],
            Page::Done => match self.result {
                Some(Ok(())) => &[("Enter", "Neu starten"), ("Q", "Beenden")],
                _ => &[("R", "Erneut versuchen"), ("Q", "Beenden")],
            },
        };
        frame.render_widget(Paragraph::new(keybar(bar)), keys);
    }

    fn on_key(&mut self, key: KeyEvent, term: &mut DefaultTerminal) -> io::Result<Flow> {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if self.page == Page::Install {
            return Ok(Flow::Continue);
        }
        if ctrl && matches!(key.code, KeyCode::Char('q' | 'c')) {
            return Ok(Flow::Quit);
        }
        let up = matches!(key.code, KeyCode::Up | KeyCode::Char('k'));
        let down = matches!(key.code, KeyCode::Down | KeyCode::Char('j'));
        match self.page {
            Page::Welcome => {
                if up || down {
                    let n = self.languages.len();
                    self.lang = if up {
                        (self.lang + n - 1) % n
                    } else {
                        (self.lang + 1) % n
                    };
                    self.apply_language();
                    self.keymap = current_keymap()
                        .and_then(|k| KEYMAPS.iter().position(|(_, c, _)| *c == k))
                        .unwrap_or_else(|| keymap_for(self.lang_code()));
                }
            }
            Page::Keyboard => {
                let n = KEYMAPS.len();
                if up {
                    self.keymap = (self.keymap + n - 1) % n;
                } else if down {
                    self.keymap = (self.keymap + 1) % n;
                }
            }
            Page::Display if !self.outputs.is_empty() => {
                let n = self.outputs[self.output].modes.len() + 1;
                let sel = &mut self.modes[self.output];
                match key.code {
                    KeyCode::Left => self.output = self.output.saturating_sub(1),
                    KeyCode::Right | KeyCode::Tab => {
                        self.output = (self.output + 1).min(self.outputs.len() - 1)
                    }
                    _ if up => *sel = (*sel + n - 1) % n,
                    _ if down => *sel = (*sel + 1) % n,
                    _ => {}
                }
            }
            Page::Components => {
                let n = self.components.len() + 1;
                if up {
                    self.comp = (self.comp + n - 1) % n;
                } else if down {
                    self.comp = (self.comp + 1) % n;
                } else if matches!(
                    key.code,
                    KeyCode::Char(' ') | KeyCode::Left | KeyCode::Right
                ) {
                    if self.comp == self.components.len() {
                        let b = BACKENDS.len();
                        self.backend = if key.code == KeyCode::Left {
                            (self.backend + b - 1) % b
                        } else {
                            (self.backend + 1) % b
                        };
                    } else {
                        let c = &mut self.components[self.comp];
                        c.on = !c.on;
                    }
                }
            }
            Page::Done => {
                match key.code {
                    KeyCode::Char('q' | 'Q') | KeyCode::Esc => return Ok(Flow::Quit),
                    KeyCode::Enter if matches!(self.result, Some(Ok(()))) => {
                        let _ = Command::new("sudo")
                            .args(["-n", "systemctl", "reboot"])
                            .status();
                        return Ok(Flow::Quit);
                    }
                    KeyCode::Char('r' | 'R') if !matches!(self.result, Some(Ok(()))) => {
                        self.page = Page::Summary;
                    }
                    _ => {}
                }
                return Ok(Flow::Continue);
            }
            _ => {}
        }
        match key.code {
            KeyCode::Enter if self.page == Page::Summary => self.start_install(term)?,
            KeyCode::Enter => self.step(1),
            KeyCode::Esc | KeyCode::Backspace => self.step(-1),
            _ => {}
        }
        Ok(Flow::Continue)
    }

    fn tick(&mut self) {
        self.poll();
    }

    fn tick_rate(&self) -> Duration {
        if self.runner.is_some() {
            Duration::from_millis(100)
        } else {
            Duration::from_millis(250)
        }
    }

    fn wants_clear(&mut self) -> bool {
        false
    }
}

impl Setup {
    fn draw_welcome(&mut self, frame: &mut Frame, area: Rect) {
        let logo_w = (area.width / 3).clamp(10, 48);
        let [logo_area, _, right] = Layout::horizontal([
            Constraint::Length(logo_w),
            Constraint::Length(2),
            Constraint::Min(20),
        ])
        .areas(area);
        let logo = self.logo_lines(logo_w, area.height.saturating_sub(1));
        let pad = logo_area.height.saturating_sub(logo.len() as u16) / 2;
        frame.render_widget(
            Paragraph::new(logo).alignment(Alignment::Center),
            Rect {
                y: logo_area.y + pad,
                height: logo_area.height - pad,
                ..logo_area
            },
        );

        let mut lines = vec![
            Line::default(),
            heading(tr("Willkommen bei TUILNUX!")),
            Line::default(),
            text(tr(
                "Der Terminal-Desktop für Arch Linux: Vollbild-Terminal mit Fenstern, Taskleiste und TUI-Apps.",
            )),
            Line::default(),
            text(tr(
                "Dieser Assistent richtet Sprache, Tastatur und Bildschirm ein und installiert danach alles Nötige.",
            )),
            Line::default(),
            heading(tr("Sprache")),
        ];
        let names: Vec<String> = self
            .languages
            .iter()
            .map(|(c, n)| format!("{n}  ({c})"))
            .collect();
        let used = lines.len() as u16 + 1;
        lines.extend(list_lines(
            &names,
            self.lang,
            right.height.saturating_sub(used),
            true,
        ));
        frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), right);
    }

    fn draw_keyboard(&self, frame: &mut Frame, area: Rect) {
        let mut lines = vec![
            text(tr(
                "Tastaturbelegung für die Konsole, die Anmeldung und grafische Programme.",
            )),
            Line::default(),
        ];
        let items: Vec<String> = KEYMAPS
            .iter()
            .map(|(label, console, xkb)| format!("{:<24} {console} / {xkb}", tr(label)))
            .collect();
        lines.extend(list_lines(
            &items,
            self.keymap,
            area.height.saturating_sub(5),
            true,
        ));
        lines.push(Line::default());
        lines.push(dim(tr("Später änderbar: tsettings → Tastatur.")));
        frame.render_widget(Paragraph::new(lines), area);
    }

    fn draw_display(&self, frame: &mut Frame, area: Rect) {
        if self.outputs.is_empty() {
            let lines = vec![
                text(tr("Kein Bildschirm über /sys/class/drm erkannt.")),
                Line::default(),
                text(tr(
                    "TUILNUX wählt beim Start automatisch die beste Auflösung und die höchste Bildrate.",
                )),
                dim(tr("Später änderbar: tsettings → Bildschirme.")),
            ];
            frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), area);
            return;
        }
        let [top, rest] = Layout::vertical([Constraint::Length(3), Constraint::Min(3)]).areas(area);
        let tabs: Vec<Span> = self
            .outputs
            .iter()
            .enumerate()
            .flat_map(|(i, o)| {
                let style = if i == self.output {
                    Style::new()
                        .fg(theme::fg())
                        .bg(theme::sel_bg())
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::new().fg(theme::dim())
                };
                let name = if o.model.is_empty() {
                    o.name.clone()
                } else {
                    format!("{} ({})", o.name, o.model)
                };
                [Span::styled(format!(" {name} "), style), Span::raw("  ")]
            })
            .collect();
        frame.render_widget(
            Paragraph::new(vec![
                text(tr(
                    "Erkannte Bildschirme – Auflösung und Bildrate (Hz) wählen:",
                )),
                Line::from(tabs),
            ]),
            top,
        );
        let out = &self.outputs[self.output];
        let mut items = vec![trf(
            "automatisch – {} (höchste Bildrate)",
            &[&out.best().map_or_else(|| "?".into(), |m| m.label())],
        )];
        items.extend(out.modes.iter().map(drm::Mode::label));
        let mut lines = list_lines(
            &items,
            self.modes[self.output],
            rest.height.saturating_sub(2),
            true,
        );
        lines.push(Line::default());
        lines.push(dim(tr("Ohne Hz-Angabe nimmt TUILNUX die höchste Bildrate der Auflösung. Später: tsettings → Bildschirme.")));
        frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), rest);
    }

    fn draw_components(&self, frame: &mut Frame, area: Rect) {
        let mut items: Vec<String> = self
            .components
            .iter()
            .enumerate()
            .map(|(i, c)| {
                let mark = if c.on { "[x]" } else { "[ ]" };
                let mut label = format!("{mark} {}", tr(c.label));
                if i == NVIDIA {
                    label.push_str(&format!(
                        "  ({})",
                        if self.nvidia_found {
                            tr("NVIDIA-Karte erkannt")
                        } else {
                            tr("keine NVIDIA-Karte gefunden")
                        }
                    ));
                }
                label
            })
            .collect();
        items.push(format!(
            "{}: ‹ {} ›",
            tr("Sitzungs-Backend"),
            tr(BACKENDS[self.backend].1)
        ));
        let [list, help] = Layout::vertical([
            Constraint::Length(items.len() as u16 + 1),
            Constraint::Min(2),
        ])
        .areas(area);
        frame.render_widget(
            Paragraph::new(list_lines(&items, self.comp, list.height, true)),
            list,
        );
        let help_text = match self.components.get(self.comp) {
            Some(c) => tr(c.help),
            None => tr("auto: labwc, wenn installiert, sonst cage oder die Linux-Konsole."),
        };
        frame.render_widget(
            Paragraph::new(vec![dim("─".repeat(help.width as usize)), text(help_text)])
                .wrap(Wrap { trim: false }),
            help,
        );
    }

    fn draw_summary(&self, frame: &mut Frame, area: Rect) {
        let plan = self.plan();
        let yes = |b: bool| if b { tr("ja") } else { tr("nein") };
        let row = |k: &str, v: String| {
            Line::from(vec![
                Span::styled(format!("  {:<22}", tr(k)), Style::new().fg(theme::dim())),
                Span::styled(v, Style::new().fg(theme::fg())),
            ])
        };
        let mut lines = vec![
            heading(tr("Bitte prüfen:")),
            Line::default(),
            row(
                "Sprache",
                format!("{} ({})", self.languages[self.lang].1, plan.locale),
            ),
            row(
                "Tastatur",
                format!(
                    "{} ({} / {})",
                    tr(KEYMAPS[self.keymap].0),
                    plan.keymap,
                    KEYMAPS[self.keymap].2
                ),
            ),
            row("Bildschirm", self.display_summary()),
        ];
        if let Some(root) = &self.root {
            lines.extend([
                row("Grafische Programme", yes(plan.graphical).into()),
                row("Anmeldebildschirm", format!("{}{}", yes(plan.greetd), if plan.enable { tr(", beim Booten") } else { "" })),
                row("~/.bashrc", yes(plan.bashrc).into()),
                row("Konsolenschrift", yes(plan.font).into()),
                row("NVIDIA", yes(plan.nvidia).into()),
                row("Tests", yes(plan.tests).into()),
                row("Backend", tr(BACKENDS[self.backend].1).into()),
                row("Quelle", root.display().to_string()),
                Line::default(),
                text(tr("Enter startet die Installation. Pakete werden mit pacman installiert (inkl. Systemaktualisierung).")),
            ]);
        } else {
            lines.extend([
                Line::default(),
                text(tr("TUILNUX ist bereits installiert – es werden nur Sprache, Tastatur und Bildschirm eingerichtet.")),
            ]);
        }
        frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), area);
    }

    fn draw_install(&self, frame: &mut Frame, area: Rect) {
        let [status, gauge, _, log] = Layout::vertical([
            Constraint::Length(2),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(1),
        ])
        .areas(area);
        let current = if self.current_step.is_empty() {
            tr("Starte …").to_string()
        } else {
            self.current_step.clone()
        };
        let headline = match &self.result {
            None => heading(trf("Installiere … {}", &[&current])),
            Some(Ok(())) => heading(tr(
                "TUILNUX ist installiert! Nach dem Neustart erscheint die Anmeldung.",
            )),
            Some(Err(e)) => Line::from(Span::styled(
                trf("Installation fehlgeschlagen: {}", &[e]),
                Style::new().fg(theme::error()).add_modifier(Modifier::BOLD),
            )),
        };
        frame.render_widget(Paragraph::new(headline).wrap(Wrap { trim: true }), status);
        let ratio = self.progress();
        let color = match &self.result {
            Some(Err(_)) => theme::error(),
            _ => theme::accent(),
        };
        frame.render_widget(
            Gauge::default()
                .ratio(ratio)
                .label(format!("{:.0} %", ratio * 100.0))
                .gauge_style(Style::new().fg(color).bg(theme::bar_bg())),
            gauge,
        );
        let height = log.height as usize;
        let lines: Vec<Line> = self
            .log
            .iter()
            .skip(self.log.len().saturating_sub(height))
            .map(|l| {
                if run::is_step(l) {
                    heading(l.clone())
                } else if l.contains("Warnung") || l.contains("Fehler") || l.contains("error") {
                    Line::from(Span::styled(l.clone(), Style::new().fg(theme::warn())))
                } else {
                    dim(l.clone())
                }
            })
            .collect();
        frame.render_widget(Paragraph::new(lines), log);
    }
}

fn main() -> anyhow::Result<()> {
    let mut root = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--root" => root = args.next().map(PathBuf::from),
            "--version" | "-V" => {
                println!("tuilnux-setup {}", env!("CARGO_PKG_VERSION"));
                return Ok(());
            }
            "--help" | "-h" => {
                println!(
                    "tuilnux-setup – grafische Ersteinrichtung von TUILNUX\n\n\
                     Aufruf: tuilnux-setup [--root QUELLORDNER]\n\n\
                     Mit Quellordner (enthält install.sh): Sprache, Tastatur, Bildschirm und\n\
                     Komponenten wählen, dann installieren. Ohne: nur Sprache, Tastatur und\n\
                     Bildschirm für ein bereits installiertes TUILNUX einrichten."
                );
                return Ok(());
            }
            other => anyhow::bail!("Unbekannte Option: {other} (siehe --help)"),
        }
    }
    let root = run::find_root(root);
    let mut app = Setup::new(root);
    tuilnux_ui::run(&mut app)?;
    if let Some(Err(e)) = &app.result {
        eprintln!("{}", trf("Installation fehlgeschlagen: {}", &[e]));
        for line in app
            .log
            .iter()
            .rev()
            .take(30)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
        {
            eprintln!("  {line}");
        }
        std::process::exit(1);
    }
    Ok(())
}
