//! Gemeinsame TUILNUX-Einstellungen.
//!
//! Alles steht in `~/.config/tuilnux/tuilnux.toml` (Pfad per `TUILNUX_CONFIG`
//! änderbar). Die Sitzung, alle TUI-Apps (Farben) und `tsettings` lesen dieselbe
//! Datei. Einstellungen für grafische Fenster, Tastatur und Maus gehören dem
//! Compositor labwc; [`apply`] erzeugt daraus dessen Konfiguration sowie die
//! GTK- und Sitzungsdateien.
//!
//! ```toml
//! prefix = "ctrl+space"
//! autostart = ["ttop"]
//!
//! [appearance]
//! theme = "nord"
//! accent = "#ff8800"
//!
//! [bar]
//! clock_format = "%H:%M"
//! battery = false
//! ```

mod generate;

use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

pub use generate::{
    Applied, apply, gtk_settings, labwc_environment, labwc_rc, labwc_theme, merge_ini, session_conf,
};

/// Umgebungsvariable, die den Pfad der Einstellungsdatei überschreibt.
pub const CONFIG_ENV: &str = "TUILNUX_CONFIG";

pub type Rgb = [u8; 3];

/// Auswahlwerte mit festem Schlüssel (in der Datei) und deutscher Beschriftung.
pub trait Choice: Sized + Copy + PartialEq + 'static {
    const ALL: &'static [Self];
    fn key(self) -> &'static str;
    fn label(self) -> &'static str;

    fn index(self) -> usize {
        Self::ALL.iter().position(|c| *c == self).unwrap_or(0)
    }
}

macro_rules! choice {
    ($(#[$meta:meta])* $name:ident [$default:ident] { $($var:ident = $key:literal, $label:literal;)+ }) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
        pub enum $name {
            $(#[serde(rename = $key)] $var,)+
        }

        impl Default for $name {
            fn default() -> Self {
                Self::$default
            }
        }

        impl Choice for $name {
            const ALL: &'static [Self] = &[$(Self::$var),+];

            fn key(self) -> &'static str {
                match self {
                    $(Self::$var => $key,)+
                }
            }

            fn label(self) -> &'static str {
                match self {
                    $(Self::$var => $label,)+
                }
            }
        }
    };
}

mod display;
mod graphics;
mod login;
mod shell;

pub use display::{
    Arrangement, DisplaySettings, ModeInfo, OutputInfo, OutputSettings, Transform, apply_outputs,
    detect_outputs, output_name_ok, parse_mode, parse_wlr_randr, wlr_randr_args,
};
pub use graphics::{
    Gpu, GraphicsSettings, Renderer, WaylandSettings, detect_gpus, nvidia_modeset, vendor_name,
};
pub use login::{
    FactorMode, LOGIN_CONFIG_ENV, LOGIN_CONFIG_FILE, LoginSettings, LogoStyle, OTP_FILE, PAM_MARK,
    SecretDisplay, U2F_KEYS_FILE, U2F_ORIGIN, faillock_conf, pam_greetd, u2f_key_count,
};
pub use shell::{
    BashSettings, PathStyle, PromptStyle, bash_init, desktop_program, gui_programs, local_bashrc,
    prompt_code, prompt_preview,
};

choice!(
    /// Farbschema als Grundlage; einzelne Farben lassen sich überschreiben.
    ThemePreset [Tuilnux] {
        Tuilnux = "tuilnux", "TUILNUX (Mint)";
        Nord = "nord", "Nord";
        Gruvbox = "gruvbox", "Gruvbox";
        Dracula = "dracula", "Dracula";
        Catppuccin = "catppuccin", "Catppuccin Mocha";
        Solarized = "solarized", "Solarized Dark";
        TokyoNight = "tokyonight", "Tokyo Night";
        Mono = "mono", "Monochrom";
    }
);

choice!(
    /// Linienart der Fensterrahmen in den TUI-Apps.
    Borders [Plain] {
        Plain = "plain", "einfach ┌─┐";
        Rounded = "rounded", "abgerundet ╭─╮";
        Double = "double", "doppelt ╔═╗";
        Thick = "thick", "dick ┏━┓";
    }
);

choice!(
    /// Wo neue TUI-Fenster erscheinen.
    Placement [Cascade] {
        Cascade = "cascade", "kaskadiert";
        Center = "center", "zentriert";
    }
);

choice!(
    /// Was ein Doppelklick auf die Titelleiste eines TUI-Fensters tut.
    TitleDoubleClick [Maximize] {
        Maximize = "maximize", "maximieren";
        Minimize = "minimize", "minimieren";
        Nothing = "nothing", "nichts";
    }
);

choice!(
    Backend [Auto] {
        Auto = "auto", "automatisch (labwc › cage › tty)";
        Labwc = "labwc", "labwc (echte GUI-Fenster)";
        Cage = "cage", "cage (Kiosk, nur Terminal)";
        Tty = "tty", "Linux-Konsole";
    }
);

choice!(
    TerminalProgram [Auto] {
        Auto = "auto", "automatisch (foot › kitty › alacritty)";
        Foot = "foot", "foot";
        Kitty = "kitty", "kitty";
        Alacritty = "alacritty", "alacritty";
    }
);

choice!(
    AccelProfile [Adaptive] {
        Adaptive = "adaptive", "adaptiv (beschleunigt)";
        Flat = "flat", "linear (ohne Beschleunigung)";
    }
);

choice!(
    /// Platzierung neuer grafischer Fenster durch labwc.
    GuiPlacement [Cascade] {
        Cascade = "cascade", "kaskadiert";
        Center = "center", "zentriert";
        Automatic = "automatic", "automatisch (freie Fläche)";
        Cursor = "cursor", "beim Mauszeiger";
    }
);

choice!(
    /// Knöpfe in der Titelleiste grafischer Fenster.
    TitleButtons [Right] {
        Right = "right", "rechts: _ □ x";
        Left = "left", "links: x _ □";
        CloseOnly = "close", "nur x";
    }
);

impl TitleButtons {
    /// labwc: `<layout>` (links:rechts).
    pub fn labwc(self) -> &'static str {
        match self {
            Self::Right => "icon:iconify,max,close",
            Self::Left => "close,iconify,max:icon",
            Self::CloseOnly => "icon:close",
        }
    }

    /// GTK: `gtk-decoration-layout` (für Programme mit eigener Titelleiste wie Firefox).
    pub fn gtk(self) -> &'static str {
        match self {
            Self::Right => "menu:minimize,maximize,close",
            Self::Left => "close,minimize,maximize:menu",
            Self::CloseOnly => "menu:close",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Appearance {
    pub theme: ThemePreset,
    pub accent: Option<String>,
    pub accent2: Option<String>,
    pub foreground: Option<String>,
    pub dim: Option<String>,
    pub warning: Option<String>,
    pub error: Option<String>,
    pub selection: Option<String>,
    pub panel: Option<String>,
    pub statusbar: Option<String>,
    pub borders: Borders,
}

impl Default for Appearance {
    fn default() -> Self {
        Self {
            theme: ThemePreset::Tuilnux,
            accent: None,
            accent2: None,
            foreground: None,
            dim: None,
            warning: None,
            error: None,
            selection: None,
            panel: None,
            statusbar: None,
            borders: Borders::Plain,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct BarSettings {
    /// Beschriftung des Menü-Abzeichens ganz links.
    pub label: String,
    pub mouse: bool,
    pub cpu: bool,
    pub ram: bool,
    pub network: bool,
    pub battery: bool,
    pub clock: bool,
    /// strftime-Format der Uhr, z. B. `%H:%M` oder `%a %d.%m. %H:%M`.
    pub clock_format: String,
    /// Höchstlänge eines Tasktitels in Zeichen.
    pub task_width: u16,
    /// Wie lange Meldungen in der Leiste stehen bleiben (Sekunden).
    pub notice_seconds: u64,
}

impl Default for BarSettings {
    fn default() -> Self {
        Self {
            label: "1".into(),
            mouse: true,
            cpu: true,
            ram: true,
            network: true,
            battery: true,
            clock: true,
            clock_format: "%Y-%m-%d %H:%M:%S".into(),
            task_width: 24,
            notice_seconds: 4,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct WindowSettings {
    /// Innengröße neuer Fenster in Spalten/Zeilen, 0 = zwei Drittel des Bildschirms.
    pub width: u16,
    pub height: u16,
    pub placement: Placement,
    pub double_click: TitleDoubleClick,
    /// Strg+Alt+1…9 holt Fenster zurück, Strg+Alt+0 den Desktop.
    pub quick_switch: bool,
    /// Grafische Programme (Firefox …) in Taskleiste, `tab` und Taskmanager zeigen.
    pub gui_in_taskbar: bool,
    /// ASCII-Linien statt Unicode-Rahmen erzwingen.
    pub ascii: bool,
}

impl Default for WindowSettings {
    fn default() -> Self {
        Self {
            width: 0,
            height: 0,
            placement: Placement::Cascade,
            double_click: TitleDoubleClick::Maximize,
            quick_switch: true,
            gui_in_taskbar: true,
            ascii: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TerminalSettings {
    pub backend: Backend,
    pub program: TerminalProgram,
    pub font: String,
    pub font_size: u8,
    /// Zeilen Verlauf pro Terminal (Desktop und Fenster).
    pub scrollback: usize,
}

impl Default for TerminalSettings {
    fn default() -> Self {
        Self {
            backend: Backend::Auto,
            program: TerminalProgram::Auto,
            font: "monospace".into(),
            font_size: 10,
            scrollback: 10_000,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct KeyboardSettings {
    /// XKB-Layout, z. B. `de` oder `de,us`; leer = Systemeinstellung.
    pub layout: String,
    pub variant: String,
    /// XKB-Optionen, z. B. `grp:alt_shift_toggle,caps:escape`.
    pub options: String,
    /// Tastenwiederholung pro Sekunde.
    pub repeat_rate: u16,
    /// Verzögerung bis zur Wiederholung in Millisekunden.
    pub repeat_delay: u16,
    pub numlock: bool,
}

impl Default for KeyboardSettings {
    fn default() -> Self {
        Self {
            layout: String::new(),
            variant: String::new(),
            options: String::new(),
            repeat_rate: 25,
            repeat_delay: 600,
            numlock: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MouseSettings {
    /// -1.0 (langsam) bis 1.0 (schnell).
    pub speed: f32,
    pub accel: AccelProfile,
    pub natural_scroll: bool,
    pub scroll_factor: f32,
    pub left_handed: bool,
    pub tap_to_click: bool,
    pub double_click_ms: u64,
    pub focus_follows_mouse: bool,
    pub raise_on_focus: bool,
}

impl Default for MouseSettings {
    fn default() -> Self {
        Self {
            speed: 0.0,
            accel: AccelProfile::Adaptive,
            natural_scroll: false,
            scroll_factor: 1.0,
            left_handed: false,
            tap_to_click: true,
            double_click_ms: 400,
            focus_follows_mouse: false,
            raise_on_focus: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct GuiSettings {
    pub buttons: TitleButtons,
    pub show_title: bool,
    pub title_font: String,
    pub title_font_size: u8,
    pub placement: GuiPlacement,
    /// Abstand in Pixeln, ab dem Fenster an Bildschirmränder einrasten (0 = aus).
    pub snap_range: u16,
    /// Fenster an den oberen Rand ziehen maximiert es.
    pub snap_top_maximize: bool,
    /// Abstand zwischen Fenstern und Rand in Pixeln.
    pub gap: u16,
    pub corner_radius: u8,
    pub drop_shadows: bool,
    /// GTK-Programme bevorzugt dunkel darstellen.
    pub prefer_dark: bool,
    /// Mauszeiger-Thema (leer = Standard) und Größe.
    pub cursor_theme: String,
    pub cursor_size: u16,
}

impl Default for GuiSettings {
    fn default() -> Self {
        Self {
            buttons: TitleButtons::Right,
            show_title: true,
            title_font: "DejaVu Sans Mono".into(),
            title_font_size: 10,
            placement: GuiPlacement::Cascade,
            snap_range: 10,
            snap_top_maximize: true,
            gap: 0,
            corner_radius: 0,
            drop_shadows: false,
            prefer_dark: true,
            cursor_theme: String::new(),
            cursor_size: 24,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Settings {
    /// Sprache der Oberfläche: `auto` (aus `LANG`), `de`, `en`, `fr`, `es` …
    pub language: String,
    /// Taste für den Manager-Modus, z. B. `ctrl+space`.
    pub prefix: String,
    /// Desktop-Shell (leer = `$SHELL`).
    pub shell: Vec<String>,
    /// Beim Start als Fenster öffnen.
    pub autostart: Vec<String>,
    pub appearance: Appearance,
    pub bar: BarSettings,
    pub windows: WindowSettings,
    pub terminal: TerminalSettings,
    pub keyboard: KeyboardSettings,
    pub mouse: MouseSettings,
    pub gui: GuiSettings,
    /// Bash: Prompt (PS1), Begrüßung, Verlauf, Aliase, GUI-Apps im Hintergrund.
    pub bash: BashSettings,
    pub displays: DisplaySettings,
    pub graphics: GraphicsSettings,
    pub wayland: WaylandSettings,
    /// Entwurf für den Anmeldebildschirm; gilt erst nach `sudo tlogin setup`.
    pub login: LoginSettings,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            language: "auto".into(),
            prefix: "ctrl+space".into(),
            shell: Vec::new(),
            autostart: Vec::new(),
            appearance: Appearance::default(),
            bar: BarSettings::default(),
            windows: WindowSettings::default(),
            terminal: TerminalSettings::default(),
            keyboard: KeyboardSettings::default(),
            mouse: MouseSettings::default(),
            gui: GuiSettings::default(),
            bash: BashSettings::default(),
            displays: DisplaySettings::default(),
            graphics: GraphicsSettings::default(),
            wayland: WaylandSettings::default(),
            login: LoginSettings::default(),
        }
    }
}

/// `~/.config/tuilnux`
pub fn config_dir() -> Option<PathBuf> {
    Some(dirs::config_dir()?.join("tuilnux"))
}

impl Settings {
    pub fn file() -> Option<PathBuf> {
        if let Some(path) = std::env::var_os(CONFIG_ENV).filter(|p| !p.is_empty()) {
            return Some(PathBuf::from(path));
        }
        Some(config_dir()?.join("tuilnux.toml"))
    }

    /// Lädt die Einstellungen; eine fehlende Datei ergibt die Standardwerte.
    pub fn load() -> Result<Self> {
        let Some(file) = Self::file() else {
            return Ok(Self::default());
        };
        match std::fs::read_to_string(&file) {
            Ok(s) => Self::parse(&s).with_context(|| file.display().to_string()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e).with_context(|| file.display().to_string()),
        }
    }

    /// Wie [`Settings::load`], fehlerhafte Dateien ergeben aber die Standardwerte.
    pub fn load_or_default() -> Self {
        Self::load().unwrap_or_default()
    }

    pub fn parse(s: &str) -> Result<Self> {
        let settings: Self = toml::from_str(s)?;
        settings.validate()?;
        Ok(settings)
    }

    pub fn to_toml(&self) -> Result<String> {
        Ok(format!(
            "# TUILNUX-Einstellungen – bearbeitbar mit `tsettings` oder von Hand.\n\
             # Nicht gesetzte Werte haben ihren Standard.\n\n{}",
            toml::to_string(self)?
        ))
    }

    pub fn save(&self) -> Result<PathBuf> {
        self.validate()?;
        let file = Self::file().context("kein Konfigurationsverzeichnis gefunden")?;
        if let Some(dir) = file.parent() {
            std::fs::create_dir_all(dir)?;
        }
        write_atomic(&file, &self.to_toml()?)?;
        Ok(file)
    }

    /// Prüft Werte, die serde nicht prüfen kann (Farben, Bereiche).
    pub fn validate(&self) -> Result<()> {
        let a = &self.appearance;
        for (name, value) in [
            ("accent", &a.accent),
            ("accent2", &a.accent2),
            ("foreground", &a.foreground),
            ("dim", &a.dim),
            ("warning", &a.warning),
            ("error", &a.error),
            ("selection", &a.selection),
            ("panel", &a.panel),
            ("statusbar", &a.statusbar),
        ] {
            if let Some(v) = value
                && parse_hex(v).is_none()
            {
                bail!("appearance.{name} = \"{v}\" ist keine Farbe im Format #rrggbb");
            }
        }
        if !(-1.0..=1.0).contains(&self.mouse.speed) {
            bail!("mouse.speed muss zwischen -1.0 und 1.0 liegen");
        }
        if !(0.1..=10.0).contains(&self.mouse.scroll_factor) {
            bail!("mouse.scroll_factor muss zwischen 0.1 und 10 liegen");
        }
        if self.terminal.font_size == 0 || self.gui.title_font_size == 0 {
            bail!("Schriftgrößen müssen größer als 0 sein");
        }
        if self.keyboard.repeat_rate == 0 {
            bail!("keyboard.repeat_rate muss größer als 0 sein");
        }
        for (name, value) in [
            ("terminal.font", &self.terminal.font),
            ("gui.title_font", &self.gui.title_font),
            ("gui.cursor_theme", &self.gui.cursor_theme),
            ("keyboard.layout", &self.keyboard.layout),
            ("keyboard.variant", &self.keyboard.variant),
            ("keyboard.options", &self.keyboard.options),
        ] {
            if value
                .chars()
                .any(|c| c.is_control() || "<>&\"'`$\\".contains(c))
            {
                bail!("{name} enthält unerlaubte Zeichen");
            }
        }
        if !clock_format_ok(&self.bar.clock_format) {
            bail!(
                "bar.clock_format = \"{}\" ist kein gültiges Zeitformat (z. B. %H:%M)",
                self.bar.clock_format
            );
        }
        if self.bar.task_width < 4 {
            bail!("bar.task_width muss mindestens 4 sein");
        }
        self.bash.validate()?;
        self.displays.validate()?;
        self.graphics.validate()?;
        self.login.validate()?;
        Ok(())
    }

    /// Farben nach Schema und eigenen Überschreibungen.
    pub fn palette(&self) -> Palette {
        let a = &self.appearance;
        let mut p = a.theme.palette();
        let pick = |slot: &mut Rgb, value: &Option<String>| {
            if let Some(rgb) = value.as_deref().and_then(parse_hex) {
                *slot = rgb;
            }
        };
        pick(&mut p.accent, &a.accent);
        pick(&mut p.accent2, &a.accent2);
        pick(&mut p.fg, &a.foreground);
        pick(&mut p.dim, &a.dim);
        pick(&mut p.warn, &a.warning);
        pick(&mut p.error, &a.error);
        pick(&mut p.selection, &a.selection);
        pick(&mut p.panel, &a.panel);
        pick(&mut p.statusbar, &a.statusbar);
        p
    }
}

/// Schreibt erst in eine temporäre Datei und benennt dann um.
/// `true`, wenn chrono das Uhrzeitformat ohne Fehler ausgeben kann.
pub fn clock_format_ok(format: &str) -> bool {
    use chrono::format::{Item, StrftimeItems};
    !StrftimeItems::new(format).any(|i| matches!(i, Item::Error))
}

pub fn write_atomic(path: &std::path::Path, content: &str) -> Result<()> {
    let tmp = path.with_extension("tmp~");
    std::fs::write(&tmp, content).with_context(|| tmp.display().to_string())?;
    std::fs::rename(&tmp, path).with_context(|| path.display().to_string())?;
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    pub fg: Rgb,
    pub dim: Rgb,
    pub accent: Rgb,
    pub accent2: Rgb,
    pub warn: Rgb,
    pub error: Rgb,
    /// Hintergrund markierter Einträge.
    pub selection: Rgb,
    /// Hintergrund von Leisten und Menüs.
    pub panel: Rgb,
    /// Hintergrund der Statusleiste.
    pub statusbar: Rgb,
}

const fn rgb(v: u32) -> Rgb {
    [(v >> 16) as u8, (v >> 8) as u8, v as u8]
}

#[allow(clippy::too_many_arguments)]
const fn palette(
    accent: u32,
    accent2: u32,
    fg: u32,
    dim: u32,
    warn: u32,
    error: u32,
    selection: u32,
    panel: u32,
    statusbar: u32,
) -> Palette {
    Palette {
        fg: rgb(fg),
        dim: rgb(dim),
        accent: rgb(accent),
        accent2: rgb(accent2),
        warn: rgb(warn),
        error: rgb(error),
        selection: rgb(selection),
        panel: rgb(panel),
        statusbar: rgb(statusbar),
    }
}

impl ThemePreset {
    pub fn palette(self) -> Palette {
        match self {
            Self::Tuilnux => palette(
                0x5fd7af, 0x87afff, 0xd0d0d0, 0x6c6c6c, 0xffd75f, 0xff5f5f, 0x264f45, 0x1c1c1c,
                0x0f1113,
            ),
            Self::Nord => palette(
                0x88c0d0, 0x81a1c1, 0xd8dee9, 0x616e88, 0xebcb8b, 0xbf616a, 0x3b4252, 0x2e3440,
                0x242933,
            ),
            Self::Gruvbox => palette(
                0xb8bb26, 0x83a598, 0xebdbb2, 0x928374, 0xfabd2f, 0xfb4934, 0x504945, 0x282828,
                0x1d2021,
            ),
            Self::Dracula => palette(
                0xbd93f9, 0x8be9fd, 0xf8f8f2, 0x6272a4, 0xf1fa8c, 0xff5555, 0x44475a, 0x282a36,
                0x21222c,
            ),
            Self::Catppuccin => palette(
                0xcba6f7, 0x89b4fa, 0xcdd6f4, 0x6c7086, 0xf9e2af, 0xf38ba8, 0x45475a, 0x1e1e2e,
                0x181825,
            ),
            Self::Solarized => palette(
                0x2aa198, 0x268bd2, 0x93a1a1, 0x657b83, 0xb58900, 0xdc322f, 0x073642, 0x002b36,
                0x00212b,
            ),
            Self::TokyoNight => palette(
                0x7aa2f7, 0xbb9af7, 0xc0caf5, 0x565f89, 0xe0af68, 0xf7768e, 0x283457, 0x1a1b26,
                0x16161e,
            ),
            Self::Mono => palette(
                0xe0e0e0, 0xb0b0b0, 0xd0d0d0, 0x707070, 0xffffff, 0xff5f5f, 0x404040, 0x1c1c1c,
                0x101010,
            ),
        }
    }
}

/// `#rrggbb` (oder `rrggbb`, `#rgb`) → RGB.
pub fn parse_hex(s: &str) -> Option<Rgb> {
    let h = s.trim().trim_start_matches('#');
    if !h.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    match h.len() {
        6 => {
            let v = u32::from_str_radix(h, 16).ok()?;
            Some(rgb(v))
        }
        3 => {
            let mut out = [0u8; 3];
            for (i, c) in h.chars().enumerate() {
                let d = c.to_digit(16)? as u8;
                out[i] = d * 17;
            }
            Some(out)
        }
        _ => None,
    }
}

pub fn hex(c: Rgb) -> String {
    format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2])
}

/// Mischt zwei Farben (`t` = 0 → `a`, 1 → `b`).
pub fn mix(a: Rgb, b: Rgb, t: f32) -> Rgb {
    let t = t.clamp(0.0, 1.0);
    let m = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    [m(a[0], b[0]), m(a[1], b[1]), m(a[2], b[2])]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_roundtrip() {
        let s = Settings::default();
        let text = s.to_toml().unwrap();
        assert_eq!(Settings::parse(&text).unwrap(), s);
        assert_eq!(Settings::parse("").unwrap(), s);
    }

    #[test]
    fn partial_files_and_errors() {
        let s = Settings::parse(
            "prefix = \"ctrl+b\"\n[appearance]\ntheme = \"nord\"\naccent = \"#ff8800\"\n\
             [bar]\nbattery = false\n[gui]\nbuttons = \"left\"",
        )
        .unwrap();
        assert_eq!(s.appearance.theme, ThemePreset::Nord);
        assert!(!s.bar.battery);
        assert!(s.bar.cpu);
        assert_eq!(s.gui.buttons, TitleButtons::Left);
        let p = s.palette();
        assert_eq!(p.accent, [0xff, 0x88, 0x00]);
        assert_eq!(p.fg, ThemePreset::Nord.palette().fg);

        assert!(Settings::parse("unbekannt = 1").is_err());
        assert!(Settings::parse("[bar]\nfoo = 1").is_err());
        assert!(Settings::parse("[appearance]\naccent = \"grün\"").is_err());
        assert!(Settings::parse("[appearance]\ntheme = \"pink\"").is_err());
        assert!(Settings::parse("[mouse]\nspeed = 3.0").is_err());
        assert!(Settings::parse("[terminal]\nfont = \"a<b\"").is_err());
    }

    #[test]
    fn colors() {
        assert_eq!(parse_hex("#5fd7af"), Some([0x5f, 0xd7, 0xaf]));
        assert_eq!(parse_hex("fff"), Some([255, 255, 255]));
        assert_eq!(parse_hex("#12345"), None);
        assert_eq!(parse_hex("#gggggg"), None);
        assert_eq!(hex([1, 2, 255]), "#0102ff");
        assert_eq!(mix([0, 0, 0], [200, 100, 50], 0.5), [100, 50, 25]);
    }

    #[test]
    fn choices_have_unique_keys() {
        fn check<T: Choice + std::fmt::Debug>() {
            let keys: Vec<_> = T::ALL.iter().map(|c| c.key()).collect();
            let mut sorted = keys.clone();
            sorted.sort();
            sorted.dedup();
            assert_eq!(sorted.len(), keys.len());
            for c in T::ALL {
                assert_eq!(T::ALL[c.index()], *c);
            }
        }
        check::<ThemePreset>();
        check::<Borders>();
        check::<Placement>();
        check::<TitleDoubleClick>();
        check::<Backend>();
        check::<TerminalProgram>();
        check::<AccelProfile>();
        check::<GuiPlacement>();
        check::<TitleButtons>();
        check::<PromptStyle>();
        check::<LogoStyle>();
        check::<SecretDisplay>();
        check::<FactorMode>();
        check::<PathStyle>();
        check::<Arrangement>();
        check::<Transform>();
        check::<Renderer>();
        check::<Gpu>();
    }
}
