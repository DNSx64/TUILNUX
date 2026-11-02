//! Alle Einstellungen als Seiten mit Feldern: Beschriftung, Hilfe und Zugriff.

use tuilnux_config::{
    AccelProfile, Arrangement, Backend, Borders, Choice, FactorMode, Gpu, GuiPlacement,
    LoginSettings, LogoStyle, OutputInfo, OutputSettings, Palette, PathStyle, Placement,
    PromptStyle, Renderer, Rgb, SecretDisplay, Settings, TerminalProgram, ThemePreset,
    TitleButtons, TitleDoubleClick, Transform, hex, parse_hex,
};
use tuilnux_ipc::{Request, Response};
use tuilnux_media::{FitMode, WallpaperConfig};
use tuilnux_ui::{Prefix, tr};

/// Alles, was tsettings bearbeitet.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Model {
    pub s: Settings,
    pub wp: WallpaperConfig,
    /// Beim Start erkannte Bildschirme (wlr-randr).
    pub outputs: Vec<OutputInfo>,
    /// Erkannte Grafikkarten.
    pub gpus: Vec<String>,
}

impl Model {
    /// Prüft Werte, die über einzelne Felder hinausgehen.
    pub fn validate(&self) -> Result<(), String> {
        self.s.validate().map_err(|e| e.to_string())?;
        Prefix::parse(&self.s.prefix).map_err(|e| format!("Manager-Taste: {e}"))?;
        if !(0.0..=1.0).contains(&self.wp.dim) {
            return Err("Abdunklung muss zwischen 0 und 1 liegen".into());
        }
        Ok(())
    }
}

pub type SetText = fn(&mut Model, String) -> Result<(), String>;

pub enum Kind {
    Bool {
        get: fn(&Model) -> bool,
        set: fn(&mut Model, bool),
    },
    Choice {
        labels: fn(&Model) -> Vec<String>,
        get: fn(&Model) -> usize,
        set: fn(&mut Model, usize),
    },
    Number {
        get: fn(&Model) -> f64,
        set: fn(&mut Model, f64),
        min: f64,
        max: f64,
        step: f64,
        decimals: usize,
        unit: &'static str,
        /// Anzeige für den Wert 0 (z. B. „automatisch“).
        zero: Option<&'static str>,
    },
    Text {
        get: fn(&Model) -> String,
        set: SetText,
        /// Anzeige für den leeren Wert.
        empty: &'static str,
    },
    Color {
        get: fn(&Model) -> Option<String>,
        set: fn(&mut Model, Option<String>),
        base: fn(&Palette) -> Rgb,
    },
    Info {
        get: fn(&Model) -> String,
    },
    /// Knopf: Enter führt `run` aus, das Ergebnis erscheint als Meldung.
    Action {
        hint: &'static str,
        run: fn(&mut Model) -> Result<String, String>,
    },
}

pub struct Field {
    pub label: &'static str,
    pub help: &'static str,
    pub kind: Kind,
}

pub struct Page {
    pub title: &'static str,
    pub help: &'static str,
    pub fields: Vec<Field>,
}

/// Wie ein Feld angezeigt wird.
#[derive(Debug, Clone, PartialEq)]
pub enum Shown {
    Bool(bool),
    Text(String),
    /// Farbe; `custom` = eigener Wert statt Schemafarbe.
    Color {
        rgb: Rgb,
        custom: bool,
    },
    Info(String),
}

impl Field {
    pub fn editable(&self) -> bool {
        !matches!(self.kind, Kind::Info { .. })
    }

    /// Bearbeiten per Texteingabe (Enter) statt Umschalten.
    pub fn needs_input(&self) -> bool {
        matches!(
            self.kind,
            Kind::Text { .. } | Kind::Color { .. } | Kind::Number { .. }
        )
    }

    pub fn shown(&self, m: &Model) -> Shown {
        match &self.kind {
            Kind::Bool { get, .. } => Shown::Bool(get(m)),
            Kind::Choice { labels, get, .. } => Shown::Text(
                labels(m)
                    .get(get(m))
                    .map_or_else(|| "?".into(), |l| tr(l).to_string()),
            ),
            Kind::Number {
                get,
                decimals,
                unit,
                zero,
                ..
            } => {
                let v = get(m);
                match zero {
                    Some(z) if v == 0.0 => Shown::Text(tr(z).to_string()),
                    _ => Shown::Text(format!("{v:.decimals$}{unit}")),
                }
            }
            Kind::Text { get, empty, .. } => {
                let v = get(m);
                Shown::Text(if v.is_empty() {
                    tr(empty).to_string()
                } else {
                    v
                })
            }
            Kind::Color { get, base, .. } => match get(m).as_deref().and_then(parse_hex) {
                Some(rgb) => Shown::Color { rgb, custom: true },
                None => Shown::Color {
                    rgb: base(&m.s.palette()),
                    custom: false,
                },
            },
            Kind::Info { get } => Shown::Info(get(m)),
            Kind::Action { hint, .. } => Shown::Text(format!("[ {} ]", tr(hint))),
        }
    }

    /// Text für die Eingabezeile.
    pub fn input_text(&self, m: &Model) -> String {
        match &self.kind {
            Kind::Number { get, decimals, .. } => format!("{:.decimals$}", get(m)),
            Kind::Text { get, .. } => get(m),
            Kind::Color { get, base, .. } => get(m).unwrap_or_else(|| hex(base(&m.s.palette()))),
            _ => String::new(),
        }
    }

    /// Schritt nach links (-1) oder rechts (+1); Schalter wechseln immer.
    pub fn step(&self, m: &mut Model, dir: i32) {
        match &self.kind {
            Kind::Bool { get, set } => {
                let v = get(m);
                set(m, !v);
            }
            Kind::Choice { labels, get, set } => {
                let n = labels(m).len().max(1) as i32;
                let i = (get(m) as i32 + dir).rem_euclid(n);
                set(m, i as usize);
            }
            Kind::Number {
                get,
                set,
                min,
                max,
                step,
                ..
            } => {
                let v = (get(m) + step * f64::from(dir)).clamp(*min, *max);
                // Rundungsfehler bei Kommaschritten (0.1 + 0.2 …) vermeiden.
                set(m, (v / step).round() * step);
            }
            _ => {}
        }
    }

    /// Übernimmt eine Texteingabe.
    pub fn submit(&self, m: &mut Model, text: &str) -> Result<(), String> {
        let text = text.trim();
        match &self.kind {
            Kind::Number { set, min, max, .. } => {
                let v: f64 = text
                    .replace(',', ".")
                    .parse()
                    .map_err(|_| format!("„{text}“ ist keine Zahl"))?;
                if !(*min..=*max).contains(&v) {
                    return Err(format!("Wert muss zwischen {min} und {max} liegen"));
                }
                set(m, v);
                Ok(())
            }
            Kind::Text { set, .. } => set(m, text.to_string()),
            Kind::Color { set, .. } => {
                if text.is_empty() {
                    set(m, None);
                    return Ok(());
                }
                let value = if text.starts_with('#') {
                    text.to_string()
                } else {
                    format!("#{text}")
                };
                let rgb = parse_hex(&value)
                    .ok_or_else(|| format!("„{text}“ ist keine Farbe (#rrggbb)"))?;
                set(m, Some(hex(rgb)));
                Ok(())
            }
            _ => Ok(()),
        }
    }

    /// Setzt das Feld auf den Standardwert zurück.
    pub fn reset(&self, m: &mut Model) {
        let d = Model::default();
        match &self.kind {
            Kind::Bool { get, set } => set(m, get(&d)),
            Kind::Choice { get, set, .. } => set(m, get(&d)),
            Kind::Number { get, set, .. } => set(m, get(&d)),
            Kind::Text { get, set, .. } => {
                let _ = set(m, get(&d));
            }
            Kind::Color { get, set, .. } => set(m, get(&d)),
            Kind::Info { .. } | Kind::Action { .. } => {}
        }
    }
}

macro_rules! toggle {
    ($label:expr, $help:expr, $($path:tt)+) => {
        Field {
            label: $label,
            help: $help,
            kind: Kind::Bool {
                get: |m| m.$($path)+,
                set: |m, v| m.$($path)+ = v,
            },
        }
    };
}

macro_rules! choice {
    ($label:expr, $help:expr, $ty:ty, $($path:tt)+) => {
        Field {
            label: $label,
            help: $help,
            kind: Kind::Choice {
                labels: |_| <$ty>::ALL.iter().map(|c| c.label().to_string()).collect(),
                get: |m| m.$($path)+.index(),
                set: |m, i| m.$($path)+ = <$ty>::ALL[i],
            },
        }
    };
}

macro_rules! number {
    ($label:expr, $help:expr, $ty:ty, $min:expr, $max:expr, $step:expr, $dec:expr, $unit:expr, $zero:expr, $($path:tt)+) => {
        Field {
            label: $label,
            help: $help,
            kind: Kind::Number {
                get: |m| m.$($path)+ as f64,
                set: |m, v| m.$($path)+ = v as $ty,
                min: $min as f64,
                max: $max as f64,
                step: $step as f64,
                decimals: $dec,
                unit: $unit,
                zero: $zero,
            },
        }
    };
}

macro_rules! text {
    ($label:expr, $help:expr, $empty:expr, $($path:tt)+) => {
        Field {
            label: $label,
            help: $help,
            kind: Kind::Text {
                get: |m| m.$($path)+.clone(),
                set: |m, v| {
                    m.$($path)+ = v;
                    Ok(())
                },
                empty: $empty,
            },
        }
    };
}

macro_rules! color {
    ($label:expr, $help:expr, $field:ident, $base:ident) => {
        Field {
            label: $label,
            help: $help,
            kind: Kind::Color {
                get: |m| m.s.appearance.$field.clone(),
                set: |m, v| m.s.appearance.$field = v,
                base: |p| p.$base,
            },
        }
    };
}

const FIT_MODES: [(FitMode, &str); 3] = [
    (FitMode::Fill, "füllen (zuschneiden)"),
    (FitMode::Fit, "einpassen (ganz sichtbar)"),
    (FitMode::Center, "zentriert (Originalgröße)"),
];

fn words(s: &str) -> Vec<String> {
    s.split_whitespace().map(str::to_string).collect()
}

/// Liste als `a; b; c` (zum Bearbeiten in einer Zeile).
fn join_list(v: &[String]) -> String {
    v.join("; ")
}

fn split_list(s: &str) -> Vec<String> {
    s.split(';')
        .map(str::trim)
        .filter(|c| !c.is_empty())
        .map(str::to_string)
        .collect()
}

/// Liste in `bash`, bearbeitet als `a; b; c`.
macro_rules! list {
    ($label:expr, $help:expr, $empty:expr, $($path:tt)+) => {
        Field {
            label: $label,
            help: $help,
            kind: Kind::Text {
                get: |m| join_list(&m.$($path)+),
                set: |m, v| {
                    m.$($path)+ = split_list(&v);
                    Ok(())
                },
                empty: $empty,
            },
        }
    };
}

/// Eigene Farbe (`Option<String>`) mit einer Schemafarbe als Grundlage.
macro_rules! own_color {
    ($label:expr, $help:expr, $base:ident, $($path:tt)+) => {
        Field {
            label: $label,
            help: $help,
            kind: Kind::Color {
                get: |m| m.$($path)+.clone(),
                set: |m, v| m.$($path)+ = v,
                base: |p| p.$base,
            },
        }
    };
}

fn info(label: &'static str, help: &'static str, get: fn(&Model) -> String) -> Field {
    Field {
        label,
        help,
        kind: Kind::Info { get },
    }
}

fn action(
    label: &'static str,
    help: &'static str,
    hint: &'static str,
    run: fn(&mut Model) -> Result<String, String>,
) -> Field {
    Field {
        label,
        help,
        kind: Kind::Action { hint, run },
    }
}

/// Öffnet ein TUILNUX-Fenster mit `command`.
fn open_window(command: Vec<String>, title: &str) -> Result<String, String> {
    match tuilnux_ipc::request_env(Request::Open {
        command,
        title: Some(title.to_string()),
        cwd: None,
        size: None,
    }) {
        Ok(Response::Error { message }) => Err(message),
        Ok(_) => Ok(format!("{title} als Fenster geöffnet")),
        Err(_) => Err("keine laufende TUILNUX-Sitzung".into()),
    }
}

fn home() -> std::path::PathBuf {
    std::env::var_os("HOME").map_or_else(|| ".".into(), std::path::PathBuf::from)
}

/// Datei im Editor aus den Shell-Einstellungen öffnen (legt sie bei Bedarf an).
fn edit_file(m: &Model, path: std::path::PathBuf, template: &str) -> Result<String, String> {
    if !path.exists() {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        std::fs::write(&path, template).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    let editor = m.s.bash.editor.trim();
    let editor = if editor.is_empty() { "tedit" } else { editor };
    let mut command = words(editor);
    command.push(path.display().to_string());
    let name = path
        .file_name()
        .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
    open_window(command, &name).map_err(|e| format!("{e} – Datei: {}", path.display()))
}

fn welcome_file(m: &Model) -> std::path::PathBuf {
    let own = m.s.bash.welcome_file.trim();
    if !own.is_empty() {
        return match own.strip_prefix("~/") {
            Some(rest) => home().join(rest),
            None => own.into(),
        };
    }
    let user = tuilnux_config::config_dir().map(|d| d.join("welcome.ASCIIA"));
    match user {
        Some(p) if p.is_file() => p,
        _ => tuilnux_config::share_file("welcome.ASCIIA")
            .unwrap_or_else(|| "/usr/share/tuilnux/welcome.ASCIIA".into()),
    }
}

// --- Bildschirme -------------------------------------------------------------

fn slot(m: &Model, i: usize) -> Option<&OutputInfo> {
    m.outputs.get(i)
}

/// Einstellungen des `i`-ten erkannten Monitors (Standardwerte, wenn keine).
fn slot_cfg(m: &Model, i: usize) -> OutputSettings {
    let Some(info) = slot(m, i) else {
        return OutputSettings::default();
    };
    m.s.displays
        .output(&info.name)
        .cloned()
        .unwrap_or_else(|| OutputSettings {
            name: info.name.clone(),
            ..OutputSettings::default()
        })
}

/// Ändert die Einstellungen des `i`-ten Monitors; Einträge mit nur
/// Standardwerten werden entfernt, damit die Datei schlank bleibt.
fn set_slot(m: &mut Model, i: usize, f: impl FnOnce(&mut OutputSettings)) {
    let Some(name) = slot(m, i).map(|o| o.name.clone()) else {
        return;
    };
    f(m.s.displays.output_mut(&name));
    let plain = OutputSettings {
        name: name.clone(),
        ..OutputSettings::default()
    };
    m.s.displays.outputs.retain(|o| *o != plain);
}

fn mode_labels(m: &Model, i: usize) -> Vec<String> {
    let mut out = vec![tr("automatisch").to_string()];
    if let Some(info) = slot(m, i) {
        let auto = if m.s.displays.max_refresh {
            info.best_mode()
        } else {
            info.preferred()
        };
        if let Some(p) = auto {
            out[0] = tuilnux_ui::trf(
                "automatisch ({}×{} @ {} Hz)",
                &[&p.width, &p.height, &format!("{:.0}", p.refresh)],
            );
        }
        out.extend(info.unique_modes().iter().map(|mode| mode.label()));
    }
    out
}

fn mode_index(m: &Model, i: usize) -> usize {
    let cfg = slot_cfg(m, i);
    let Some(info) = slot(m, i) else {
        return 0;
    };
    let Some((w, h, r)) = tuilnux_config::parse_mode(&cfg.mode) else {
        return 0;
    };
    let Some(found) = info.find_mode(w, h, r) else {
        return 0;
    };
    info.unique_modes()
        .iter()
        .position(|mode| mode.key() == found.key())
        .map_or(0, |p| p + 1)
}

fn set_mode(m: &mut Model, i: usize, choice: usize) {
    let key = match choice {
        0 => String::new(),
        n => slot(m, i)
            .and_then(|info| info.unique_modes().get(n - 1).map(|mode| mode.key()))
            .unwrap_or_default(),
    };
    set_slot(m, i, |o| o.mode = key);
}

macro_rules! output_fields {
    ($i:literal, $title:literal) => {
        vec![
            info(
                $title,
                "Erkannter Bildschirm (Name, Modell, aktueller Modus).",
                |m| {
                    slot(m, $i).map_or("nicht angeschlossen".into(), |o| {
                        format!("{} – {}", o.name, o.summary())
                    })
                },
            ),
            Field {
                label: "  eingeschaltet",
                help: "Bildschirm benutzen (aus = schwarz, Fenster wandern weg).",
                kind: Kind::Bool {
                    get: |m| slot_cfg(m, $i).enabled,
                    set: |m, v| set_slot(m, $i, |o| o.enabled = v),
                },
            },
            Field {
                label: "  Auflösung",
                help: "Auflösung und Bildwiederholrate (Hz); automatisch = empfohlene Auflösung, höchste Hz.",
                kind: Kind::Choice {
                    labels: |m| mode_labels(m, $i),
                    get: |m| mode_index(m, $i),
                    set: |m, c| set_mode(m, $i, c),
                },
            },
            Field {
                label: "  Skalierung",
                help: "1 = normal, 2 = doppelt so groß (HiDPI/4K); auch 1.25, 1.5 …",
                kind: Kind::Number {
                    get: |m| f64::from(slot_cfg(m, $i).scale),
                    set: |m, v| set_slot(m, $i, |o| o.scale = v as f32),
                    min: 0.5,
                    max: 4.0,
                    step: 0.25,
                    decimals: 2,
                    unit: "×",
                    zero: None,
                },
            },
            Field {
                label: "  Drehung",
                help: "Hochformat-Monitore um 90° oder 270° drehen.",
                kind: Kind::Choice {
                    labels: |_| {
                        Transform::ALL
                            .iter()
                            .map(|c| c.label().to_string())
                            .collect()
                    },
                    get: |m| slot_cfg(m, $i).transform.index(),
                    set: |m, c| set_slot(m, $i, |o| o.transform = Transform::ALL[c]),
                },
            },
            Field {
                label: "  Position X",
                help: "Nur bei Anordnung „manuell“: linke Kante in logischen Pixeln.",
                kind: Kind::Number {
                    get: |m| f64::from(slot_cfg(m, $i).x),
                    set: |m, v| set_slot(m, $i, |o| o.x = v as i32),
                    min: -16384.0,
                    max: 16384.0,
                    step: 10.0,
                    decimals: 0,
                    unit: " px",
                    zero: None,
                },
            },
            Field {
                label: "  Position Y",
                help: "Nur bei Anordnung „manuell“: obere Kante in logischen Pixeln.",
                kind: Kind::Number {
                    get: |m| f64::from(slot_cfg(m, $i).y),
                    set: |m, v| set_slot(m, $i, |o| o.y = v as i32),
                    min: -16384.0,
                    max: 16384.0,
                    step: 10.0,
                    decimals: 0,
                    unit: " px",
                    zero: None,
                },
            },
            Field {
                label: "  Adaptive Sync",
                help: "FreeSync/G-Sync für diesen Monitor (wenn er es kann).",
                kind: Kind::Bool {
                    get: |m| slot_cfg(m, $i).adaptive_sync,
                    set: |m, v| set_slot(m, $i, |o| o.adaptive_sync = v),
                },
            },
        ]
    };
}

fn desktop_labels(m: &Model) -> Vec<String> {
    let mut out = vec!["automatisch".to_string()];
    out.extend(
        m.outputs
            .iter()
            .map(|o| format!("{} ({})", o.name, o.product())),
    );
    let own = &m.s.displays.desktop;
    if !own.is_empty() && !m.outputs.iter().any(|o| &o.name == own) {
        out.push(format!("{own} (nicht angeschlossen)"));
    }
    out
}

fn desktop_index(m: &Model) -> usize {
    let own = &m.s.displays.desktop;
    if own.is_empty() {
        return 0;
    }
    m.outputs
        .iter()
        .position(|o| &o.name == own)
        .map_or(m.outputs.len() + 1, |p| p + 1)
}

fn set_desktop(m: &mut Model, i: usize) {
    if i == 0 {
        m.s.displays.desktop.clear();
    } else if let Some(o) = m.outputs.get(i - 1) {
        m.s.displays.desktop = o.name.clone();
    }
}

fn display_page(m: &Model) -> Page {
    let mut fields = vec![
        Field {
            label: "Desktop auf",
            help: "Bildschirm für Terminal-Desktop und Statusleiste (wirkt nach erneutem Anmelden).",
            kind: Kind::Choice {
                labels: desktop_labels,
                get: desktop_index,
                set: set_desktop,
            },
        },
        choice!(
            "Anordnung",
            "Wie mehrere Bildschirme nebeneinander liegen.",
            Arrangement,
            s.displays.arrangement
        ),
        toggle!(
            "Höchste Bildrate",
            "Bei „automatisch“: empfohlene Auflösung mit den meisten Hz (z. B. 144 statt 60).",
            s.displays.max_refresh
        ),
    ];
    let slots = [
        output_fields!(0, "Monitor 1"),
        output_fields!(1, "Monitor 2"),
        output_fields!(2, "Monitor 3"),
        output_fields!(3, "Monitor 4"),
    ];
    let n = m.outputs.len().clamp(1, slots.len());
    for slot in slots.into_iter().take(n) {
        fields.extend(slot);
    }
    fields.push(action(
        "Jetzt anwenden",
        "Bildschirm-Einstellungen sofort testen (ohne zu speichern).",
        "Enter",
        |m| {
            tuilnux_config::apply_outputs(&m.s.displays, true)
                .map(|n| n.unwrap_or_else(|| "nichts zu tun".into()))
                .map_err(|e| format!("{e:#}"))
        },
    ));
    fields.push(info(
        "Werkzeug",
        "Auf der Shell: tuilnux outputs (anzeigen), wlr-randr (alle Optionen).",
        |m| {
            if m.outputs.is_empty() {
                "keine Bildschirme erkannt (wlr-randr, Wayland nötig)".into()
            } else {
                format!("{} Bildschirm(e) erkannt", m.outputs.len())
            }
        },
    ));
    Page {
        title: "Bildschirme",
        help: "Auflösung, Skalierung, Drehung und Anordnung (Wayland/labwc). Super+Umschalt+←/→ schiebt Fenster.",
        fields,
    }
}

fn shell_page() -> Page {
    Page {
        title: "Shell & Prompt",
        help: "Die komplette Bash-Konfiguration (tuilnux shell-init). Wirkt in neu geöffneten Shells.",
        fields: vec![
            choice!(
                "Prompt-Stil",
                "Aussehen der Eingabeaufforderung (PS1).",
                PromptStyle,
                s.bash.prompt
            ),
            text!(
                "Eigenes PS1",
                "Nur bei Prompt-Stil „eigenes PS1“, z. B. \\u@\\h:\\w\\$  (Bash-Escapes erlaubt).",
                "(leer)",
                s.bash.prompt_custom
            ),
            choice!(
                "Ordneranzeige",
                "Wie der aktuelle Ordner im Prompt steht.",
                PathStyle,
                s.bash.path_style
            ),
            toggle!("Benutzer zeigen", "user im Prompt.", s.bash.prompt_user),
            toggle!("Rechner zeigen", "@host im Prompt.", s.bash.prompt_host),
            toggle!(
                "Git-Zweig",
                "Zweig des Git-Repos im aktuellen Ordner.",
                s.bash.prompt_git
            ),
            toggle!(
                "Fehlercode",
                "Rückgabewert des letzten Befehls, wenn er nicht 0 war.",
                s.bash.prompt_status
            ),
            toggle!("Uhrzeit", "Uhrzeit vor dem Prompt.", s.bash.prompt_time),
            own_color!(
                "Farbe Benutzer",
                "Farbe für user@host (#rrggbb, leer = Farbschema).",
                accent,
                s.bash.prompt_user_color
            ),
            own_color!(
                "Farbe Ordner",
                "Farbe des Ordners (#rrggbb, leer = Farbschema).",
                accent2,
                s.bash.prompt_path_color
            ),
            text!(
                "Prompt-Zeichen",
                "Zeichen vor der Eingabe, z. B. ❯ $ » λ",
                "(keins)",
                s.bash.prompt_symbol
            ),
            toggle!(
                "Fenstertitel",
                "Titel in der Taskleiste auf user@host: ordner setzen.",
                s.bash.terminal_title
            ),
            number!(
                "Verlaufsgröße",
                "So viele Befehle merkt sich der Verlauf (HISTSIZE).",
                u32,
                100,
                1_000_000,
                1000,
                0,
                " Befehle",
                None,
                s.bash.history_size
            ),
            toggle!(
                "Doppelte ignorieren",
                "Gleiche Befehle und solche mit Leerzeichen vorne nicht speichern.",
                s.bash.history_ignore_dups
            ),
            toggle!(
                "Verlauf teilen",
                "Verlauf sofort zwischen allen offenen Shells teilen.",
                s.bash.history_shared
            ),
            toggle!(
                "Tab-Vervollständigung",
                "bash-completion laden.",
                s.bash.completion
            ),
            toggle!(
                "Farbiges ls/grep",
                "ls, grep, diff und ip farbig.",
                s.bash.color_aliases
            ),
            list!(
                "Aliase",
                "NAME=BEFEHL mit ; getrennt, z. B. ll=ls -lah; gs=git status",
                "(keine)",
                s.bash.aliases
            ),
            list!(
                "Umgebungsvariablen",
                "NAME=WERT mit ; getrennt, z. B. LANG=de_DE.UTF-8",
                "(keine)",
                s.bash.env
            ),
            list!(
                "PATH ergänzen",
                "Ordner mit ; getrennt, z. B. ~/.local/bin; ~/bin",
                "(nichts)",
                s.bash.path
            ),
            text!(
                "Editor",
                "$EDITOR und $VISUAL, z. B. tedit, nano, nvim.",
                "(nicht setzen)",
                s.bash.editor
            ),
            toggle!(
                "GUI im Hintergrund",
                "Grafische Programme aus der Shell im Hintergrund starten; Ausgabe: tab --shell NAME.",
                s.bash.gui_background
            ),
            list!(
                "Weitere GUI-Programme",
                "Zusätzlich im Hintergrund starten (; getrennt), z. B. gimp; obs",
                "(automatisch)",
                s.bash.gui_apps
            ),
            list!(
                "Nie im Hintergrund",
                "Diese Programme laufen immer im Vordergrund (; getrennt).",
                "(keine)",
                s.bash.gui_exclude
            ),
            action(
                "bashrc.local bearbeiten",
                "Eigene Ergänzungen, werden zuletzt geladen (~/.config/tuilnux/bashrc.local).",
                "Enter: öffnen",
                |m| {
                    let path = tuilnux_config::local_bashrc().ok_or("kein Konfigurationsordner")?;
                    edit_file(
                        m,
                        path,
                        "# Eigene Bash-Ergänzungen – geladen nach der TUILNUX-Shell.\n",
                    )
                },
            ),
            action(
                "Erzeugtes Skript",
                "Zeigt die komplette erzeugte bashrc (tuilnux shell-init bash) in einem Fenster.",
                "Enter: anzeigen",
                |_| {
                    open_window(
                        vec![
                            "sh".into(),
                            "-c".into(),
                            "tuilnux shell-init bash | ${PAGER:-less}".into(),
                        ],
                        "shell-init",
                    )
                },
            ),
            info(
                "Einbindung",
                "So lädt die ~/.bashrc die TUILNUX-Shell (macht der Installer).",
                |_| r#"eval "$(tuilnux shell-init bash)""#.into(),
            ),
        ],
    }
}

fn welcome_page() -> Page {
    Page {
        title: "Willkommen",
        help: "Begrüßung beim Öffnen der Shell: ASCIIA-Bild (asciia) und eigener Text.",
        fields: vec![
            toggle!(
                "Begrüßung zeigen",
                "ASCIIA-Begrüßung beim Öffnen der Shell.",
                s.bash.welcome
            ),
            toggle!(
                "Auch in Fenstern",
                "Auch in neuen Terminal-Fenstern, nicht nur im Desktop.",
                s.bash.welcome_in_windows
            ),
            text!(
                "ASCIIA-Datei",
                "Eigene .ASCIIA-Datei (Bild/Animation, siehe asciia --help); leer = Standard.",
                "(Standard)",
                s.bash.welcome_file
            ),
            text!(
                "Zusatztext",
                "Zeile unter dem Bild, z. B. Willkommen zurück, Jonas!",
                "(keiner)",
                s.bash.welcome_text
            ),
            info("Verwendete Datei", "Diese Datei zeigt die Shell an.", |m| {
                welcome_file(m).display().to_string()
            }),
            action(
                "Bild bearbeiten",
                "Öffnet die eigene welcome.ASCIIA im Editor (kopiert vorher die mitgelieferte).",
                "Enter: öffnen",
                |m| {
                    let current = welcome_file(m);
                    let target = if m.s.bash.welcome_file.trim().is_empty() {
                        tuilnux_config::config_dir()
                            .ok_or("kein Konfigurationsordner")?
                            .join("welcome.ASCIIA")
                    } else {
                        current.clone()
                    };
                    let template = std::fs::read_to_string(&current).unwrap_or_default();
                    edit_file(m, target, &template)
                },
            ),
            action(
                "Vorschau",
                "Zeigt die Begrüßung in einem neuen Fenster.",
                "Enter: anzeigen",
                |m| {
                    let file = welcome_file(m).display().to_string();
                    let mut script = String::from("asciia \"$0\" --once; ");
                    if !m.s.bash.welcome_text.trim().is_empty() {
                        script.push_str("printf '%s\\n' \"$1\"; ");
                    }
                    script.push_str("read -rsn1 -p '(Taste drücken)'");
                    open_window(
                        vec![
                            "bash".into(),
                            "-c".into(),
                            script,
                            file,
                            m.s.bash.welcome_text.trim().to_string(),
                        ],
                        "Willkommen",
                    )
                },
            ),
        ],
    }
}

fn graphics_page() -> Page {
    Page {
        title: "Grafik & NVIDIA",
        help: "Renderer und Treiber-Umgebung für labwc und GUI-Apps. Wirkt nach erneutem Anmelden.",
        fields: vec![
            info(
                "Grafikkarten",
                "Erkannte Grafikkarten (/sys/class/drm).",
                |m| {
                    if m.gpus.is_empty() {
                        "keine erkannt".into()
                    } else {
                        m.gpus.join(" · ")
                    }
                },
            ),
            info(
                "NVIDIA-Modesetting",
                "Für Wayland mit NVIDIA nötig: nvidia_drm.modeset=1 (macht install.sh --nvidia).",
                |_| match tuilnux_config::nvidia_modeset() {
                    Some(true) => "an (gut)".into(),
                    Some(false) => "AUS – nvidia_drm.modeset=1 setzen!".into(),
                    None => "kein NVIDIA-Treiber geladen".into(),
                },
            ),
            choice!(
                "Treiber",
                "Für welchen Treiber die Umgebung gesetzt wird; automatisch erkennt NVIDIA beim Start.",
                Gpu,
                s.graphics.gpu
            ),
            choice!(
                "Renderer",
                "Renderer von labwc; Software hilft in VMs ohne 3D.",
                Renderer,
                s.graphics.renderer
            ),
            toggle!(
                "Hardware-Mauszeiger",
                "Aus = Software-Zeiger (gegen unsichtbare/flackernde Zeiger bei NVIDIA/VMs).",
                s.graphics.hardware_cursors
            ),
            toggle!(
                "NVIDIA GBM",
                "GBM_BACKEND=nvidia-drm und GLX von NVIDIA setzen.",
                s.graphics.nvidia_gbm
            ),
            toggle!(
                "NVIDIA Video (VA-API)",
                "Videobeschleunigung über nvidia-vaapi-driver (Paket libva-nvidia-driver).",
                s.graphics.nvidia_vaapi
            ),
            text!(
                "VA-API-Treiber",
                "LIBVA_DRIVER_NAME, z. B. iHD, radeonsi, nvidia; leer = automatisch.",
                "(automatisch)",
                s.graphics.vaapi_driver
            ),
            text!(
                "Grafikkarte erzwingen",
                "Nur diese Karte(n) benutzen, z. B. /dev/dri/card1 (Laptops mit 2 GPUs).",
                "(alle)",
                s.graphics.drm_device
            ),
            toggle!(
                "Adaptive Sync",
                "FreeSync/G-Sync erlauben (zusätzlich je Monitor einschalten).",
                s.graphics.adaptive_sync
            ),
            toggle!(
                "Tearing erlauben",
                "Für Vollbild-Spiele: geringere Latenz, kann reißen.",
                s.graphics.allow_tearing
            ),
            toggle!(
                "OpenGL in Software",
                "LIBGL_ALWAYS_SOFTWARE=1 – nur für Problemfälle.",
                s.graphics.software_gl
            ),
        ],
    }
}

fn wayland_page() -> Page {
    Page {
        title: "Wayland",
        help: "GUI-Apps nativ unter Wayland statt Xwayland starten. Wirkt nach erneutem Anmelden.",
        fields: vec![
            toggle!(
                "Firefox/Thunderbird",
                "MOZ_ENABLE_WAYLAND=1",
                s.wayland.firefox
            ),
            toggle!(
                "Qt-Apps",
                "QT_QPA_PLATFORM=wayland;xcb (Rückfall auf X11).",
                s.wayland.qt
            ),
            toggle!("GTK-Apps", "GDK_BACKEND=wayland,x11", s.wayland.gtk),
            toggle!(
                "Electron/Chromium",
                "VS Code, Discord, Chromium … (ELECTRON_OZONE_PLATFORM_HINT=auto).",
                s.wayland.electron
            ),
            toggle!(
                "SDL-Spiele",
                "SDL_VIDEODRIVER=wayland – manche ältere Spiele brauchen X11.",
                s.wayland.sdl
            ),
            toggle!(
                "Java-Apps",
                "_JAVA_AWT_WM_NONREPARENTING=1 gegen graue Fenster.",
                s.wayland.java
            ),
            toggle!("Clutter-Apps", "CLUTTER_BACKEND=wayland", s.wayland.clutter),
            info(
                "Xwayland",
                "Alte X11-Programme laufen über Xwayland weiter.",
                |_| {
                    if std::path::Path::new("/usr/bin/Xwayland").exists() {
                        "installiert".into()
                    } else {
                        format!(
                            "nicht installiert ({})",
                            tuilnux_config::install_hint(tuilnux_config::Package::Xwayland)
                        )
                    }
                },
            ),
            info("Sitzung", "Wayland-Display der laufenden Sitzung.", |_| {
                std::env::var("WAYLAND_DISPLAY").unwrap_or_else(|_| "keins".into())
            }),
        ],
    }
}

// --- Anmeldung und Konto -----------------------------------------------------

/// Farbe als `#rrggbb`-Text (Pflichtwert).
macro_rules! hex_text {
    ($label:expr, $help:expr, $($path:tt)+) => {
        Field {
            label: $label,
            help: $help,
            kind: Kind::Text {
                get: |m| m.$($path)+.clone(),
                set: |m, v| {
                    let v = if v.starts_with('#') { v } else { format!("#{v}") };
                    let rgb = parse_hex(&v).ok_or_else(|| format!("„{v}“ ist keine Farbe (#rrggbb)"))?;
                    m.$($path)+ = hex(rgb);
                    Ok(())
                },
                empty: "(Standard)",
            },
        }
    };
}

/// Befehl in einem Fenster ausführen und danach auf eine Taste warten.
fn run_window(script: &str, args: &[String], title: &str) -> Result<String, String> {
    let mut command = vec![
        "bash".to_string(),
        "-c".to_string(),
        format!("{script}; echo; read -rsn1 -p '(Taste drücken)'"),
    ];
    command.extend(args.iter().cloned());
    open_window(command, title)
}

fn runtime_dir() -> std::path::PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR")
        .filter(|d| !d.is_empty())
        .map_or_else(std::env::temp_dir, std::path::PathBuf::from)
}

/// Schreibt den Entwurf für tlogin in eine Datei (für `--from`/`--config`).
fn login_draft(m: &Model) -> Result<String, String> {
    m.s.login.validate().map_err(|e| e.to_string())?;
    let path = runtime_dir().join("tuilnux-login.toml");
    let text = m.s.login.to_toml().map_err(|e| e.to_string())?;
    std::fs::write(&path, text).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(path.display().to_string())
}

fn system_login_state(m: &Model) -> String {
    let file = LoginSettings::file();
    if !file.is_file() {
        return if m.s.login == LoginSettings::default() {
            "Standard (nicht eingerichtet)".into()
        } else {
            "nicht eingerichtet – Entwurf mit „Systemweit übernehmen“ aktivieren".into()
        };
    }
    match LoginSettings::load_system() {
        Ok(s) if s == m.s.login => format!(
            "übernommen · Schlüssel {} · Code {}",
            s.key.label(),
            s.otp.label()
        ),
        Ok(_) => "Entwurf weicht ab – „Systemweit übernehmen“".into(),
        Err(_) => format!("{} ist fehlerhaft", file.display()),
    }
}

/// Ausgabe von `tuilnux-boot status` ohne das Präfix „Start: “.
fn boot_mode_state() -> String {
    std::process::Command::new("tuilnux-boot")
        .arg("status")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| {
            let out = String::from_utf8_lossy(&o.stdout);
            let line = out.lines().next().unwrap_or_default();
            line.strip_prefix("Start: ").unwrap_or(line).to_string()
        })
        .unwrap_or_else(|| "tuilnux-boot nicht gefunden".into())
}

fn login_page() -> Page {
    Page {
        title: "Anmeldung",
        help: "Anmeldebildschirm (tlogin): Aussehen, Schutz vor Ausprobieren, Sicherheitsschlüssel und Handy-Code. Gilt erst nach „Systemweit übernehmen“ (sudo).",
        fields: vec![
            info(
                "Systemzustand",
                "/etc/tuilnux/login.toml im Vergleich zu diesem Entwurf.",
                system_login_state,
            ),
            info(
                "Start nach dem Booten",
                "Wie TUILNUX nach dem Einschalten startet (tuilnux-boot).",
                |_| boot_mode_state(),
            ),
            action(
                "Direkt in TUILNUX booten",
                "Ohne greetd und ohne Passwort: die Konsole meldet dich automatisch an, TUILNUX startet sofort. Ab dem nächsten Neustart.",
                "Enter: sudo",
                |_| run_window("sudo tuilnux-boot autologin", &[], "Startmodus"),
            ),
            action(
                "Text-Login, dann TUILNUX",
                "Ohne greetd: normaler Login der Konsole, danach startet TUILNUX automatisch.",
                "Enter: sudo",
                |_| run_window("sudo tuilnux-boot console", &[], "Startmodus"),
            ),
            action(
                "Anmeldebildschirm tlogin",
                "greetd mit tlogin – nötig für Sicherheitsschlüssel und Handy-Code.",
                "Enter: sudo",
                |_| run_window("sudo tuilnux-boot greetd", &[], "Startmodus"),
            ),
            action(
                "Nicht automatisch starten",
                "Kein Autostart; ein vorheriger Anmeldemanager (gdm, sddm …) wird wieder aktiviert.",
                "Enter: sudo",
                |_| run_window("sudo tuilnux-boot off", &[], "Startmodus"),
            ),
            hex_text!(
                "Linienfarbe",
                "Farbe von Logo, Rahmen und Text (Vorlage: Cyan #22c8d6).",
                s.login.accent
            ),
            hex_text!(
                "Hintergrund",
                "Hintergrundfarbe (Vorlage: Schwarz #000000).",
                s.login.background
            ),
            choice!(
                "Logo",
                "Schriftzug über der Anmeldebox.",
                LogoStyle,
                s.login.logo
            ),
            toggle!(
                "Rechnername zeigen",
                "Titel user@arch:~$ statt user:~$.",
                s.login.show_host
            ),
            toggle!(
                "Uhr zeigen",
                "Datum und Uhrzeit unten rechts.",
                s.login.show_clock
            ),
            text!(
                "Hinweistext",
                "Text unter der Box, z. B. „Nur für befugte Personen“.",
                "(keiner)",
                s.login.notice
            ),
            choice!(
                "Passwortanzeige",
                "Feste Länge oder unsichtbar verrät nicht, wie lang das Passwort ist.",
                SecretDisplay,
                s.login.secret_display
            ),
            toggle!(
                "Feststelltaste warnen",
                "Warnung, wenn die Feststelltaste an ist.",
                s.login.caps_warning
            ),
            toggle!(
                "Benutzer merken",
                "Benutzername vorausfüllen (verrät ihn aber jedem am Bildschirm).",
                s.login.remember_user
            ),
            number!(
                "Versuche bis Pause",
                "Nach so vielen Fehlversuchen muss man warten; 0 = nie.",
                u32,
                0,
                100,
                1,
                0,
                "",
                Some("nie"),
                s.login.max_attempts
            ),
            number!(
                "Pause",
                "Wartezeit, verdoppelt sich bei jeder weiteren Pause.",
                u32,
                0,
                3600,
                5,
                0,
                " s",
                Some("keine"),
                s.login.lockout_seconds
            ),
            number!(
                "Inaktivität",
                "Eingaben nach so vielen Sekunden ohne Taste löschen.",
                u32,
                0,
                3600,
                10,
                0,
                " s",
                Some("nie"),
                s.login.idle_timeout
            ),
            toggle!(
                "Ausschalten erlauben",
                "reboot/poweroff am Anmeldebildschirm.",
                s.login.allow_power
            ),
            toggle!(
                "Sitzungswahl (F2)",
                "Andere Sitzung (Terminal, Bash) wählbar.",
                s.login.allow_session_switch
            ),
            number!(
                "Kontosperre nach",
                "pam_faillock: Fehlversuche bis das Konto gesperrt wird (gilt für alle Anmeldungen); 0 = aus.",
                u32,
                0,
                100,
                1,
                0,
                " Versuchen",
                Some("aus"),
                s.login.faillock_deny
            ),
            number!(
                "Kontosperre Dauer",
                "Wie lange gesperrt; 0 = bis root mit faillock --reset entsperrt.",
                u32,
                0,
                86400,
                60,
                0,
                " s",
                Some("bis root entsperrt"),
                s.login.faillock_unlock
            ),
            choice!(
                "Sicherheitsschlüssel",
                "FIDO2/U2F (YubiKey …): zusätzlich = nach dem Passwort berühren; statt Passwort = nur berühren. Koppeln: Seite Konto.",
                FactorMode,
                s.login.key
            ),
            choice!(
                "Handy-Code",
                "Code aus einer Authenticator-App (per QR-Code gekoppelt): zusätzlich oder statt Passwort. Koppeln: Seite Konto.",
                FactorMode,
                s.login.otp
            ),
            action(
                "Anmeldung testen",
                "Zeigt den Anmeldebildschirm mit diesem Entwurf (Demo, Passwort tuilnux, Strg+C beendet).",
                "Enter: anzeigen",
                |m| {
                    let path = login_draft(m)?;
                    open_window(
                        vec![
                            "tlogin".into(),
                            "--demo".into(),
                            "--config".into(),
                            path,
                            "--cmd".into(),
                            "true".into(),
                        ],
                        "Anmeldung (Vorschau)",
                    )
                },
            ),
            action(
                "Systemweit übernehmen",
                "Schreibt login.toml, PAM und faillock über sudo tlogin setup; fehlende Pakete (pam-u2f, libpam-google-authenticator) werden angeboten.",
                "Enter: sudo",
                |m| {
                    let path = login_draft(m)?;
                    run_window(
                        "sudo tlogin setup --from \"$0\"",
                        &[path],
                        "Anmeldung einrichten",
                    )
                },
            ),
            action(
                "Auslieferungszustand",
                "PAM und faillock zurücksetzen, login.toml löschen (sudo tlogin setup --reset).",
                "Enter: sudo",
                |_| run_window("sudo tlogin setup --reset", &[], "Anmeldung zurücksetzen"),
            ),
        ],
    }
}

/// Eintrag aus /etc/passwd: (uid, gid, gecos, home, shell).
fn passwd_entry(user: &str) -> Option<(String, String, String, String, String)> {
    let text = std::fs::read_to_string("/etc/passwd").ok()?;
    text.lines().find_map(|l| {
        let f: Vec<&str> = l.split(':').collect();
        (f.len() >= 7 && f[0] == user).then(|| {
            (
                f[2].to_string(),
                f[3].to_string(),
                f[4].split(',').next().unwrap_or("").to_string(),
                f[5].to_string(),
                f[6].to_string(),
            )
        })
    })
}

fn current_user() -> String {
    std::env::var("USER")
        .or_else(|_| std::env::var("LOGNAME"))
        .unwrap_or_else(|_| "?".into())
}

fn user_groups(user: &str) -> Vec<String> {
    let primary = passwd_entry(user).map(|e| e.1);
    std::fs::read_to_string("/etc/group")
        .unwrap_or_default()
        .lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split(':').collect();
            let member = f.get(3).is_some_and(|m| m.split(',').any(|u| u == user));
            let main = primary.as_deref().is_some_and(|g| f.get(2) == Some(&g));
            (f.len() >= 3 && (member || main)).then(|| f[0].to_string())
        })
        .collect()
}

fn account_page() -> Page {
    Page {
        title: "Konto",
        help: "Dein Benutzerkonto: Passwort, Name, Login-Shell, Sicherheitsschlüssel und Handy-Kopplung per QR-Code.",
        fields: vec![
            info("Benutzer", "Angemeldeter Benutzer.", |_| {
                let user = current_user();
                match passwd_entry(&user) {
                    Some((uid, _, name, _, _)) if !name.is_empty() => {
                        format!("{user} ({name}) · UID {uid}")
                    }
                    Some((uid, ..)) => format!("{user} · UID {uid}"),
                    None => user,
                }
            }),
            info("Gruppen", "wheel = darf sudo benutzen.", |_| {
                let g = user_groups(&current_user());
                if g.is_empty() {
                    "-".into()
                } else {
                    g.join(", ")
                }
            }),
            info("Persönlicher Ordner", "", |_| home().display().to_string()),
            info("Login-Shell", "Shell beim Anmelden (chsh).", |_| {
                passwd_entry(&current_user()).map_or("-".into(), |e| e.4)
            }),
            info(
                "Gekoppelte Schlüssel",
                "Gekoppelte FIDO2/U2F-Schlüssel (~/.config/Yubico/u2f_keys).",
                |m| {
                    let content =
                        std::fs::read_to_string(home().join(tuilnux_config::U2F_KEYS_FILE))
                            .unwrap_or_default();
                    let n = tuilnux_config::u2f_key_count(&content, &current_user());
                    format!("{n} gekoppelt · Anmeldung: {}", m.s.login.key.label())
                },
            ),
            info(
                "Handy-Kopplung",
                "Per QR-Code gekoppelte Authenticator-App (~/.google_authenticator).",
                |m| {
                    let paired = home().join(tuilnux_config::OTP_FILE).is_file();
                    format!(
                        "{} · Anmeldung: {}",
                        if paired {
                            "gekoppelt"
                        } else {
                            "nicht gekoppelt"
                        },
                        m.s.login.otp.label()
                    )
                },
            ),
            action(
                "Passwort ändern",
                "Öffnet passwd in einem Fenster.",
                "Enter: ändern",
                |_| run_window("passwd", &[], "Passwort ändern"),
            ),
            action(
                "Anzeigename",
                "Vollständiger Name (chfn -f).",
                "Enter: ändern",
                |_| {
                    run_window(
                        "read -rp 'Neuer Name: ' n && chfn -f \"$n\"",
                        &[],
                        "Anzeigename",
                    )
                },
            ),
            action(
                "Login-Shell ändern",
                "chsh -s, z. B. /bin/bash oder /usr/bin/zsh (siehe /etc/shells).",
                "Enter: ändern",
                |_| {
                    run_window(
                        "cat /etc/shells; read -rp 'Neue Shell: ' s && chsh -s \"$s\"",
                        &[],
                        "Shell wechseln",
                    )
                },
            ),
            action(
                "Handy koppeln (QR)",
                "Zeigt einen QR-Code für Aegis, 2FAS, Google Authenticator …",
                "Enter: QR-Code",
                |_| run_window("tuilnux otp setup", &[], "Handy koppeln"),
            ),
            action(
                "Handy entkoppeln",
                "Entfernt ~/.google_authenticator.",
                "Enter: löschen",
                |_| run_window("tuilnux otp remove", &[], "Handy-Kopplung"),
            ),
            action(
                "Schlüssel hinzufügen",
                "Sicherheitsschlüssel einstecken, dann berühren (braucht pam-u2f).",
                "Enter: koppeln",
                |_| run_window("tuilnux key add", &[], "Sicherheitsschlüssel"),
            ),
            action(
                "Schlüssel entfernen",
                "Entfernt alle gekoppelten Schlüssel dieses Benutzers.",
                "Enter: entfernen",
                |_| run_window("tuilnux key remove", &[], "Sicherheitsschlüssel"),
            ),
        ],
    }
}

/// Sprachen zur Auswahl: `auto` zuerst, dann alle Sprachpakete.
fn language_options() -> Vec<(String, String)> {
    let mut v = vec![(
        "auto".to_string(),
        tr("automatisch (Systemsprache)").to_string(),
    )];
    v.extend(tuilnux_ui::i18n::available());
    v
}

fn language_index(code: &str) -> usize {
    language_options()
        .iter()
        .position(|(c, _)| c == code)
        .unwrap_or(0)
}

fn language_code(i: usize) -> String {
    language_options()
        .get(i)
        .map_or_else(|| "auto".into(), |(c, _)| c.clone())
}

fn system_locale() -> String {
    std::fs::read_to_string("/etc/locale.conf")
        .ok()
        .and_then(|t| {
            t.lines()
                .find_map(|l| l.trim().strip_prefix("LANG=").map(str::to_string))
        })
        .unwrap_or_else(|| tr("nicht gesetzt").into())
}

fn language_page() -> Page {
    Page {
        title: "Sprache",
        help: "Sprache aller TUILNUX-Programme. Eigene Sprachpakete: ~/.config/tuilnux/lang/<code>.toml",
        fields: vec![
            Field {
                label: "Oberfläche",
                help: "Wird sofort übernommen. „automatisch“ folgt LANG bzw. /etc/locale.conf.",
                kind: Kind::Choice {
                    labels: |_| language_options().into_iter().map(|(_, n)| n).collect(),
                    get: |m| language_index(&m.s.language),
                    set: |m, i| {
                        m.s.language = language_code(i);
                        tuilnux_ui::i18n::init(&m.s.language);
                    },
                },
            },
            Field {
                label: "Anmeldebildschirm",
                help: "Gilt nach „Anmeldung → Systemweit übernehmen“.",
                kind: Kind::Choice {
                    labels: |_| language_options().into_iter().map(|(_, n)| n).collect(),
                    get: |m| language_index(&m.s.login.language),
                    set: |m, i| m.s.login.language = language_code(i),
                },
            },
            Field {
                label: "Systemsprache",
                help: "LANG aus /etc/locale.conf (Datum, Programme, Konsole).",
                kind: Kind::Info {
                    get: |_| system_locale(),
                },
            },
            Field {
                label: "Systemsprache setzen",
                help: "Erzeugt die Locale und setzt LANG systemweit (sudo). Gilt nach Neuanmeldung.",
                kind: Kind::Action {
                    hint: "Enter",
                    run: |m| {
                        let code = tuilnux_ui::i18n::resolve(&m.s.language);
                        run_window(
                            "sudo sed -i \"s/^#\\s*$0 /$0 /\" /etc/locale.gen && sudo locale-gen && sudo localectl set-locale LANG=\"$0\" && echo && echo \"LANG=$0\"",
                            &[tuilnux_ui::i18n::locale_for(&code)],
                            "Systemsprache",
                        )
                    },
                },
            },
            Field {
                label: "Sprachpakete",
                help: "Ordner: /usr/share/tuilnux/lang, /etc/tuilnux/lang, ~/.config/tuilnux/lang",
                kind: Kind::Info {
                    get: |_| {
                        tuilnux_ui::i18n::available()
                            .into_iter()
                            .map(|(c, _)| c)
                            .collect::<Vec<_>>()
                            .join(", ")
                    },
                },
            },
        ],
    }
}

pub fn pages(m: &Model) -> Vec<Page> {
    vec![
        Page {
            title: "Darstellung",
            help: "Farben und Rahmen aller TUILNUX-Programme. Änderungen sind sofort hier sichtbar.",
            fields: vec![
                choice!(
                    "Farbschema",
                    "Grundpalette; einzelne Farben unten überschreiben sie.",
                    ThemePreset,
                    s.appearance.theme
                ),
                choice!(
                    "Rahmen",
                    "Linienart der Fensterrahmen in den TUI-Apps.",
                    Borders,
                    s.appearance.borders
                ),
                color!(
                    "Akzentfarbe",
                    "Fokus, Auswahl, Tasten. Enter: #rrggbb eingeben, leer = Schemafarbe.",
                    accent,
                    accent
                ),
                color!(
                    "Zweitakzent",
                    "Hervorhebungen (Verzeichnisse, Überschriften).",
                    accent2,
                    accent2
                ),
                color!("Textfarbe", "Normaler Text.", foreground, fg),
                color!("Gedimmt", "Hinweise, inaktive Rahmen, Trenner.", dim, dim),
                color!("Warnung", "Warnungen und hohe Last.", warning, warn),
                color!("Fehler", "Fehler und kritische Werte.", error, error),
                color!(
                    "Auswahl",
                    "Hintergrund ausgewählter Zeilen.",
                    selection,
                    selection
                ),
                color!(
                    "Leisten",
                    "Hintergrund von Kopf- und Tastenleisten.",
                    panel,
                    panel
                ),
                color!(
                    "Statusleiste",
                    "Hintergrund der unteren TUILNUX-Leiste.",
                    statusbar,
                    statusbar
                ),
                toggle!(
                    "Nur ASCII-Zeichen",
                    "Rahmen aus + - | statt Unicode (für die Linux-Konsole).",
                    s.windows.ascii
                ),
            ],
        },
        Page {
            title: "Statusleiste",
            help: "Die Leiste am unteren Bildschirmrand.",
            fields: vec![
                text!(
                    "Menü-Beschriftung",
                    "Text im Abzeichen ganz links.",
                    "(leer)",
                    s.bar.label
                ),
                toggle!(
                    "Maus-Anzeige",
                    "Zeigt, ob die Maus gerade von TUILNUX gefangen ist.",
                    s.bar.mouse
                ),
                toggle!("CPU", "Prozessorlast.", s.bar.cpu),
                toggle!("RAM", "Arbeitsspeicher.", s.bar.ram),
                toggle!("Netzwerk", "Verbindungsstatus.", s.bar.network),
                toggle!("Akku", "Ladestand (nur wenn vorhanden).", s.bar.battery),
                toggle!("Uhr", "Datum und Uhrzeit.", s.bar.clock),
                text!(
                    "Uhrformat",
                    "strftime: %H:%M, %a %d.%m. %H:%M, %Y-%m-%d %H:%M:%S …",
                    "(leer)",
                    s.bar.clock_format
                ),
                number!(
                    "Taskbreite",
                    "Höchstlänge eines Fenstertitels in der Leiste (Zeichen).",
                    u16,
                    4,
                    80,
                    1,
                    0,
                    " Zeichen",
                    None,
                    s.bar.task_width
                ),
                number!(
                    "Meldungsdauer",
                    "Wie lange Meldungen in der Leiste stehen bleiben.",
                    u64,
                    1,
                    60,
                    1,
                    0,
                    " s",
                    None,
                    s.bar.notice_seconds
                ),
            ],
        },
        Page {
            title: "Fenster",
            help: "TUI-Fenster in TUILNUX und die Fensterverwaltung.",
            fields: vec![
                text!(
                    "Manager-Taste",
                    "Startet den Manager-Modus, z. B. ctrl+space, ctrl+a, alt+w.",
                    "(leer)",
                    s.prefix
                ),
                number!(
                    "Breite neuer Fenster",
                    "Innenbreite in Spalten; 0 = zwei Drittel des Bildschirms.",
                    u16,
                    0,
                    400,
                    2,
                    0,
                    " Spalten",
                    Some("automatisch"),
                    s.windows.width
                ),
                number!(
                    "Höhe neuer Fenster",
                    "Innenhöhe in Zeilen; 0 = zwei Drittel des Bildschirms.",
                    u16,
                    0,
                    200,
                    1,
                    0,
                    " Zeilen",
                    Some("automatisch"),
                    s.windows.height
                ),
                choice!(
                    "Platzierung",
                    "Wo neue Fenster erscheinen.",
                    Placement,
                    s.windows.placement
                ),
                choice!(
                    "Doppelklick Titel",
                    "Aktion bei Doppelklick auf die Titelleiste.",
                    TitleDoubleClick,
                    s.windows.double_click
                ),
                toggle!(
                    "Strg+Alt+Zahl",
                    "Strg+Alt+1…9 holt Fenster zurück, Strg+Alt+0 den Desktop.",
                    s.windows.quick_switch
                ),
                toggle!(
                    "GUI in Taskleiste",
                    "Firefox & Co. in Leiste, Taskmanager und tab anzeigen.",
                    s.windows.gui_in_taskbar
                ),
            ],
        },
        Page {
            title: "GUI-Fenster",
            help: "Grafische Programme unter labwc (Firefox, Discord, GIMP …).",
            fields: vec![
                choice!(
                    "Titelknöpfe",
                    "Minimieren/Maximieren/Schließen – auch in Firefox (GTK).",
                    TitleButtons,
                    s.gui.buttons
                ),
                toggle!(
                    "Titel anzeigen",
                    "Fenstertitel in der Titelleiste.",
                    s.gui.show_title
                ),
                text!(
                    "Titelschrift",
                    "Schriftart der Titelleiste (fc-list zeigt alle).",
                    "(Standard)",
                    s.gui.title_font
                ),
                number!(
                    "Titelschriftgröße",
                    "Punktgröße der Titelschrift.",
                    u8,
                    4,
                    48,
                    1,
                    0,
                    " pt",
                    None,
                    s.gui.title_font_size
                ),
                choice!(
                    "Platzierung",
                    "Wo labwc neue Fenster öffnet.",
                    GuiPlacement,
                    s.gui.placement
                ),
                number!(
                    "Einrasten",
                    "Abstand zum Rand, ab dem Fenster einrasten; 0 = aus.",
                    u16,
                    0,
                    100,
                    1,
                    0,
                    " px",
                    Some("aus"),
                    s.gui.snap_range
                ),
                toggle!(
                    "Oben maximieren",
                    "Fenster an den oberen Rand ziehen maximiert es.",
                    s.gui.snap_top_maximize
                ),
                number!(
                    "Fensterabstand",
                    "Lücke zwischen Fenstern und Bildschirmrand.",
                    u16,
                    0,
                    100,
                    1,
                    0,
                    " px",
                    None,
                    s.gui.gap
                ),
                number!(
                    "Eckenradius",
                    "Abrundung der Titelleiste.",
                    u8,
                    0,
                    20,
                    1,
                    0,
                    " px",
                    None,
                    s.gui.corner_radius
                ),
                toggle!(
                    "Schatten",
                    "Schlagschatten unter Fenstern.",
                    s.gui.drop_shadows
                ),
                toggle!(
                    "Dunkles Design",
                    "GTK-Programme bevorzugt dunkel darstellen.",
                    s.gui.prefer_dark
                ),
                text!(
                    "Mauszeiger-Thema",
                    "z. B. Adwaita; leer = Standard.",
                    "(Standard)",
                    s.gui.cursor_theme
                ),
                number!(
                    "Mauszeigergröße",
                    "Größe des Mauszeigers in Pixeln.",
                    u16,
                    8,
                    128,
                    4,
                    0,
                    " px",
                    None,
                    s.gui.cursor_size
                ),
            ],
        },
        Page {
            title: "Terminal",
            help: "Das Vollbild-Terminal, in dem TUILNUX läuft. Wirkt nach erneutem Anmelden.",
            fields: vec![
                choice!(
                    "Backend",
                    "labwc für echte GUI-Fenster, cage als Kiosk, tty ohne Grafik.",
                    Backend,
                    s.terminal.backend
                ),
                choice!(
                    "Terminalprogramm",
                    "Welches Terminal den Desktop anzeigt.",
                    TerminalProgram,
                    s.terminal.program
                ),
                text!(
                    "Schrift",
                    "Schriftart des Terminals, z. B. monospace, JetBrains Mono.",
                    "(Standard)",
                    s.terminal.font
                ),
                number!(
                    "Schriftgröße",
                    "Punktgröße der Terminalschrift.",
                    u8,
                    4,
                    48,
                    1,
                    0,
                    " pt",
                    None,
                    s.terminal.font_size
                ),
                number!(
                    "Verlauf",
                    "Zeilen Scrollback pro Terminal (Shift+Bild↑).",
                    usize,
                    100,
                    1_000_000,
                    1000,
                    0,
                    " Zeilen",
                    None,
                    s.terminal.scrollback
                ),
                Field {
                    label: "Shell",
                    help: "Programm des Desktops, z. B. bash, zsh, fish; leer = $SHELL.",
                    kind: Kind::Text {
                        get: |m| m.s.shell.join(" "),
                        set: |m, v| {
                            m.s.shell = words(&v);
                            Ok(())
                        },
                        empty: "($SHELL)",
                    },
                },
            ],
        },
        Page {
            title: "Tastatur",
            help: "Tastaturlayout und Wiederholung (labwc). Leer = Systemeinstellung.",
            fields: vec![
                text!(
                    "Layout",
                    "XKB-Layout, z. B. de, us oder de,us.",
                    "(System)",
                    s.keyboard.layout
                ),
                text!(
                    "Variante",
                    "z. B. nodeadkeys, neo; leer = Standard.",
                    "(Standard)",
                    s.keyboard.variant
                ),
                text!(
                    "Optionen",
                    "z. B. grp:alt_shift_toggle,caps:escape.",
                    "(keine)",
                    s.keyboard.options
                ),
                number!(
                    "Wiederholrate",
                    "Zeichen pro Sekunde beim Gedrückthalten.",
                    u16,
                    1,
                    100,
                    1,
                    0,
                    " /s",
                    None,
                    s.keyboard.repeat_rate
                ),
                number!(
                    "Verzögerung",
                    "Wartezeit bis zur Wiederholung.",
                    u16,
                    100,
                    2000,
                    50,
                    0,
                    " ms",
                    None,
                    s.keyboard.repeat_delay
                ),
                toggle!(
                    "Num-Lock",
                    "Ziffernblock beim Start einschalten.",
                    s.keyboard.numlock
                ),
            ],
        },
        Page {
            title: "Maus",
            help: "Maus und Touchpad (labwc/libinput) sowie Klickverhalten in TUILNUX.",
            fields: vec![
                number!(
                    "Geschwindigkeit",
                    "-1 langsam … 0 normal … 1 schnell.",
                    f32,
                    -1.0,
                    1.0,
                    0.1,
                    1,
                    "",
                    None,
                    s.mouse.speed
                ),
                choice!(
                    "Beschleunigung",
                    "Zeiger beschleunigt bei schnellen Bewegungen.",
                    AccelProfile,
                    s.mouse.accel
                ),
                toggle!(
                    "Natürliches Scrollen",
                    "Inhalt folgt den Fingern (wie am Handy).",
                    s.mouse.natural_scroll
                ),
                number!(
                    "Scrollfaktor",
                    "Wie weit ein Rad-Schritt scrollt.",
                    f32,
                    0.1,
                    10.0,
                    0.1,
                    1,
                    "×",
                    None,
                    s.mouse.scroll_factor
                ),
                toggle!(
                    "Linkshänder",
                    "Linke und rechte Taste tauschen.",
                    s.mouse.left_handed
                ),
                toggle!(
                    "Tippen zum Klicken",
                    "Touchpad: Antippen klickt.",
                    s.mouse.tap_to_click
                ),
                number!(
                    "Doppelklickzeit",
                    "Höchstabstand zweier Klicks eines Doppelklicks.",
                    u64,
                    100,
                    1500,
                    50,
                    0,
                    " ms",
                    None,
                    s.mouse.double_click_ms
                ),
                toggle!(
                    "Fokus folgt Maus",
                    "GUI-Fenster unter dem Zeiger bekommt den Fokus.",
                    s.mouse.focus_follows_mouse
                ),
                toggle!(
                    "Beim Fokus anheben",
                    "Fokussierte GUI-Fenster nach vorne holen.",
                    s.mouse.raise_on_focus
                ),
            ],
        },
        Page {
            title: "Hintergrundbild",
            help: "Bild hinter dem Desktop-Terminal (auch: tuilnux wallpaper <bild>).",
            fields: vec![
                Field {
                    label: "Bild",
                    help: "Pfad zu PNG/JPEG/GIF/WebP; leer = kein Bild.",
                    kind: Kind::Text {
                        get: |m| {
                            m.wp.path
                                .as_ref()
                                .map(|p| p.display().to_string())
                                .unwrap_or_default()
                        },
                        set: |m, v| {
                            if v.is_empty() {
                                m.wp.path = None;
                                return Ok(());
                            }
                            let path = match (v.strip_prefix("~/"), std::env::var_os("HOME")) {
                                (Some(rest), Some(home)) => std::path::Path::new(&home).join(rest),
                                _ => v.into(),
                            };
                            if !path.is_file() {
                                return Err(format!("{} gibt es nicht", path.display()));
                            }
                            m.wp.path = Some(path);
                            Ok(())
                        },
                        empty: "(keins)",
                    },
                },
                number!(
                    "Abdunklung",
                    "0 = Originalhelligkeit, 1 = schwarz; hält Text lesbar.",
                    f32,
                    0.0,
                    1.0,
                    0.05,
                    2,
                    "",
                    None,
                    wp.dim
                ),
                Field {
                    label: "Modus",
                    help: "Wie das Bild den Bildschirm ausfüllt.",
                    kind: Kind::Choice {
                        labels: |_| FIT_MODES.iter().map(|(_, l)| l.to_string()).collect(),
                        get: |m| {
                            FIT_MODES
                                .iter()
                                .position(|(f, _)| *f == m.wp.mode)
                                .unwrap_or(0)
                        },
                        set: |m, i| m.wp.mode = FIT_MODES[i].0,
                    },
                },
            ],
        },
        Page {
            title: "Autostart",
            help: "Programme, die beim Anmelden als TUILNUX-Fenster starten.",
            fields: vec![Field {
                label: "Programme",
                help: "Befehle mit ; getrennt, z. B. ttop; tfiles ~/Bilder",
                kind: Kind::Text {
                    get: |m| m.s.autostart.join("; "),
                    set: |m, v| {
                        m.s.autostart = v
                            .split(';')
                            .map(str::trim)
                            .filter(|c| !c.is_empty())
                            .map(str::to_string)
                            .collect();
                        Ok(())
                    },
                    empty: "(keine)",
                },
            }],
        },
        shell_page(),
        welcome_page(),
        display_page(m),
        graphics_page(),
        wayland_page(),
        language_page(),
        login_page(),
        account_page(),
        Page {
            title: "Info",
            help: "Wo die Einstellungen liegen und was gerade läuft.",
            fields: vec![
                Field {
                    label: "Version",
                    help: "",
                    kind: Kind::Info {
                        get: |_| env!("CARGO_PKG_VERSION").to_string(),
                    },
                },
                Field {
                    label: "Einstellungsdatei",
                    help: "Lässt sich auch von Hand bearbeiten; danach: tuilnux reload",
                    kind: Kind::Info {
                        get: |_| Settings::file().map_or("-".into(), |f| f.display().to_string()),
                    },
                },
                Field {
                    label: "Backend",
                    help: "Backend der laufenden Sitzung.",
                    kind: Kind::Info {
                        get: |_| {
                            std::env::var("TUILNUX_BACKEND").unwrap_or_else(|_| "unbekannt".into())
                        },
                    },
                },
                Field {
                    label: "Sitzung",
                    help: "Ohne Sitzung wirken die Werte beim nächsten Start.",
                    kind: Kind::Info {
                        get: |_| {
                            if std::env::var_os("TUILNUX_SOCK").is_some() {
                                "verbunden (Speichern lädt live neu)".into()
                            } else {
                                "nicht in TUILNUX gestartet".into()
                            }
                        },
                    },
                },
                Field {
                    label: "Compositor",
                    help: "labwc übernimmt Fenster-, Tastatur- und Mauswerte beim Speichern.",
                    kind: Kind::Info {
                        get: |_| {
                            if std::env::var_os("LABWC_PID").is_some() {
                                "labwc läuft".into()
                            } else {
                                "kein labwc".into()
                            }
                        },
                    },
                },
            ],
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn find<'a>(pages: &'a [Page], label: &str) -> &'a Field {
        pages
            .iter()
            .flat_map(|p| &p.fields)
            .find(|f| f.label == label)
            .unwrap()
    }

    #[test]
    fn toggles_choices_and_numbers() {
        let pages = pages(&Model::default());
        let mut m = Model::default();
        find(&pages, "CPU").step(&mut m, 1);
        assert!(!m.s.bar.cpu);
        find(&pages, "Farbschema").step(&mut m, -1);
        assert_eq!(m.s.appearance.theme, ThemePreset::Mono);
        find(&pages, "Farbschema").step(&mut m, 1);
        assert_eq!(m.s.appearance.theme, ThemePreset::Tuilnux);
        let speed = find(&pages, "Geschwindigkeit");
        for _ in 0..30 {
            speed.step(&mut m, 1);
        }
        assert_eq!(m.s.mouse.speed, 1.0);
        speed.step(&mut m, -1);
        assert!((m.s.mouse.speed - 0.9).abs() < 1e-6);
        let width = find(&pages, "Breite neuer Fenster");
        assert_eq!(width.shown(&m), Shown::Text("automatisch".into()));
        width.step(&mut m, 1);
        assert_eq!(width.shown(&m), Shown::Text("2 Spalten".into()));
        width.reset(&mut m);
        assert_eq!(m.s.windows.width, 0);
    }

    #[test]
    fn input_is_parsed() {
        let pages = pages(&Model::default());
        let mut m = Model::default();
        let accent = find(&pages, "Akzentfarbe");
        assert!(accent.submit(&mut m, "zz").is_err());
        accent.submit(&mut m, "FF8800").unwrap();
        assert_eq!(m.s.appearance.accent.as_deref(), Some("#ff8800"));
        assert_eq!(
            accent.shown(&m),
            Shown::Color {
                rgb: [255, 136, 0],
                custom: true
            }
        );
        accent.submit(&mut m, "").unwrap();
        assert_eq!(m.s.appearance.accent, None);

        let delay = find(&pages, "Verzögerung");
        assert!(delay.submit(&mut m, "5").is_err());
        delay.submit(&mut m, "250").unwrap();
        assert_eq!(m.s.keyboard.repeat_delay, 250);
        find(&pages, "Scrollfaktor").submit(&mut m, "1,5").unwrap();
        assert_eq!(m.s.mouse.scroll_factor, 1.5);

        find(&pages, "Shell").submit(&mut m, " zsh  -l ").unwrap();
        assert_eq!(m.s.shell, ["zsh", "-l"]);
        find(&pages, "Programme")
            .submit(&mut m, "ttop; ; tfiles ~/Bilder;")
            .unwrap();
        assert_eq!(m.s.autostart, ["ttop", "tfiles ~/Bilder"]);
        assert!(
            find(&pages, "Bild")
                .submit(&mut m, "/gibt/es/nicht.png")
                .is_err()
        );
    }

    #[test]
    fn login_and_account_labels_are_unique() {
        let pages = pages(&Model::default());
        let new = |p: &&Page| p.title == "Anmeldung" || p.title == "Konto";
        let mut seen = std::collections::HashSet::new();
        for f in pages.iter().filter(new).flat_map(|p| &p.fields) {
            assert!(seen.insert(f.label), "doppelt: {}", f.label);
        }
        for f in pages.iter().filter(|p| !new(p)).flat_map(|p| &p.fields) {
            assert!(!seen.contains(f.label), "doppelt: {}", f.label);
        }
    }

    #[test]
    fn login_page_edits_draft() {
        let pages = pages(&Model::default());
        let mut m = Model::default();
        let line = find(&pages, "Linienfarbe");
        assert!(line.submit(&mut m, "nope").is_err());
        line.submit(&mut m, "FF0000").unwrap();
        assert_eq!(m.s.login.accent, "#ff0000");
        find(&pages, "Handy-Code").step(&mut m, 1);
        assert_eq!(m.s.login.otp, FactorMode::Second);
        find(&pages, "Sicherheitsschlüssel").step(&mut m, 1);
        assert_eq!(m.s.login.key, FactorMode::Second);
        assert!(m.validate().is_ok());
        find(&pages, "Handy-Code").step(&mut m, 1);
        find(&pages, "Sicherheitsschlüssel").step(&mut m, 1);
        assert!(m.validate().is_err());
    }
    #[test]
    fn validation_catches_bad_values() {
        let pages = pages(&Model::default());
        let mut m = Model::default();
        assert!(m.validate().is_ok());
        find(&pages, "Uhrformat").submit(&mut m, "%Q").unwrap();
        assert!(m.validate().is_err());
        find(&pages, "Uhrformat").reset(&mut m);
        find(&pages, "Manager-Taste")
            .submit(&mut m, "ctrl+")
            .unwrap();
        assert!(m.validate().unwrap_err().contains("Manager-Taste"));
    }

    #[test]
    fn every_field_has_a_working_default() {
        let d = Model::default();
        for page in pages(&d) {
            for f in &page.fields {
                let mut m = d.clone();
                f.step(&mut m, 1);
                f.step(&mut m, -1);
                if !matches!(f.kind, Kind::Bool { .. }) {
                    assert_eq!(m, d, "{} kehrt nicht zurück", f.label);
                }
                f.reset(&mut m);
                assert_eq!(m, d, "{} lässt sich nicht zurücksetzen", f.label);
            }
        }
    }

    fn with_outputs() -> Model {
        let json = r#"[{"name":"DP-1","make":"Dell","model":"U2720Q","enabled":true,
            "modes":[{"width":3840,"height":2160,"refresh":60.0,"preferred":true,"current":true},
                     {"width":1920,"height":1080,"refresh":60.0}]},
            {"name":"HDMI-A-1","enabled":true,
            "modes":[{"width":1920,"height":1080,"refresh":144.0,"preferred":true,"current":true}]}]"#;
        Model {
            outputs: tuilnux_config::parse_wlr_randr(json).unwrap(),
            ..Model::default()
        }
    }

    #[test]
    fn monitors_are_configured_per_slot() {
        let d = with_outputs();
        let pages = pages(&d);
        let page = pages.iter().find(|p| p.title == "Bildschirme").unwrap();
        let labels: Vec<_> = page.fields.iter().map(|f| f.label).collect();
        assert_eq!(labels.iter().filter(|l| **l == "  Auflösung").count(), 2);

        let mut m = d.clone();
        let mode = page
            .fields
            .iter()
            .find(|f| f.label == "  Auflösung")
            .unwrap();
        assert!(
            matches!(mode.shown(&m), Shown::Text(t) if t.starts_with("automatisch (3840×2160 @ 60 Hz)"))
        );
        mode.step(&mut m, 1);
        assert_eq!(
            m.s.displays.output("DP-1").unwrap().mode,
            "3840x2160@60.000"
        );
        mode.step(&mut m, 1);
        assert_eq!(
            m.s.displays.output("DP-1").unwrap().mode,
            "1920x1080@60.000"
        );
        assert!(m.validate().is_ok());
        mode.step(&mut m, 1);
        assert_eq!(m, d, "Standardwerte entfernen den Eintrag");

        let desktop = &page.fields[0];
        desktop.step(&mut m, 1);
        desktop.step(&mut m, 1);
        assert_eq!(m.s.displays.desktop, "HDMI-A-1");
        desktop.reset(&mut m);
        assert_eq!(m, d);

        for f in &page.fields {
            let mut m = d.clone();
            f.step(&mut m, 1);
            f.step(&mut m, -1);
            if !matches!(f.kind, Kind::Bool { .. }) {
                assert_eq!(m, d, "{} kehrt nicht zurück", f.label);
            }
            f.reset(&mut m);
            assert_eq!(m, d, "{} lässt sich nicht zurücksetzen", f.label);
        }
    }

    #[test]
    fn shell_lists_are_edited_as_one_line() {
        let d = Model::default();
        let pages = pages(&d);
        let mut m = d.clone();
        assert_eq!(
            find(&pages, "Aliase").input_text(&m),
            "ll=ls -lah; la=ls -A"
        );
        find(&pages, "Aliase")
            .submit(&mut m, "gs=git status;  ; ll=ls -l")
            .unwrap();
        assert_eq!(m.s.bash.aliases, ["gs=git status", "ll=ls -l"]);
        find(&pages, "Aliase").submit(&mut m, "kaputt").unwrap();
        assert!(m.validate().is_err());
        find(&pages, "Aliase").reset(&mut m);
        find(&pages, "Prompt-Stil").step(&mut m, 1);
        assert_eq!(m.s.bash.prompt, PromptStyle::TwoLine);
        find(&pages, "Farbe Benutzer")
            .submit(&mut m, "ff0000")
            .unwrap();
        assert_eq!(m.s.bash.prompt_user_color.as_deref(), Some("#ff0000"));
        assert!(m.validate().is_ok());
    }
}
