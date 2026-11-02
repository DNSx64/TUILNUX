//! Aus den Einstellungen erzeugte Dateien für labwc, GTK und `tuilnux-session`.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result};

use crate::{Choice, Settings, config_dir, hex, mix, write_atomic};

fn yes(b: bool) -> &'static str {
    if b { "yes" } else { "no" }
}

/// `rc.xml` für labwc. Das Desktop-Terminal (app_id `tuilnux-desktop`) bleibt
/// rahmenlos, im Vollbild (startet `tuilnux-session` so) und immer ganz unten.
pub fn labwc_rc(s: &Settings) -> String {
    let g = &s.gui;
    let k = &s.keyboard;
    let m = &s.mouse;
    let snap = g.snap_range;
    format!(
        r#"<?xml version="1.0"?>
<!--
  labwc-Konfiguration für das TUILNUX-Backend "labwc".
  Erzeugt aus ~/.config/tuilnux/tuilnux.toml: tsettings schreibt diese Datei
  nach ~/.config/tuilnux/labwc (hat Vorrang vor der mitgelieferten Fassung);
  Änderungen von Hand werden beim nächsten Speichern in tsettings überschrieben.

  Das Vollbild-Terminal mit tuilnux (app_id "tuilnux-desktop") ist der Desktop:
  ohne Rahmen, im Vollbild und immer ganz unten. Grafische Programme (Firefox,
  Discord, …) öffnen sich darüber als normale Fenster, die man an der
  Titelleiste verschieben und am Rand in der Größe ändern kann. Den Streifen
  der Statusleiste hält TUILNUX per Layer-Shell (exklusive Zone) frei.
-->
<labwc_config>
  <core>
    <decoration>server</decoration>
    <gap>{gap}</gap>
    <adaptiveSync>{adaptive}</adaptiveSync>
    <allowTearing>{tearing}</allowTearing>
  </core>

  <placement>
    <policy>{placement}</policy>
  </placement>

  <theme>
    <titlebar>
      <layout>{layout}</layout>
      <showTitle>{show_title}</showTitle>
    </titlebar>
    <cornerRadius>{radius}</cornerRadius>
    <dropShadows>{shadows}</dropShadows>
    <font>
      <name>{font}</name>
      <size>{font_size}</size>
      <weight>bold</weight>
    </font>
  </theme>

  <desktops number="1" />

  <focus>
    <followMouse>{follow}</followMouse>
    <raiseOnFocus>{raise}</raiseOnFocus>
  </focus>

  <snapping>
    <range>
      <inner>{snap}</inner>
      <outer>{snap}</outer>
    </range>
    <topMaximize>{top_max}</topMaximize>
  </snapping>

  <!--
    Standard-Tastenkürzel von labwc, u. a.:
      Alt+Tab   Fenster wechseln (holt auch minimierte zurück)
      Alt+F4    Fenster schließen
      Super+A   maximieren
      Alt+Leertaste  Fenstermenü
    Dazu von TUILNUX:
      Super+Unten  minimieren      Super+Oben  maximieren umschalten
      Super+Umschalt+Links/Rechts  Fenster auf den anderen Bildschirm
      Super+F      Programme suchen und starten (tuilnux search)
    Strg+Leertaste bleibt frei für den TUILNUX-Manager.
  -->
  <keyboard>
    <numlock>{numlock}</numlock>
    <repeatRate>{rate}</repeatRate>
    <repeatDelay>{delay}</repeatDelay>
    <default />
    <keybind key="W-Down">
      <action name="Iconify" />
    </keybind>
    <keybind key="W-Up">
      <action name="ToggleMaximize" />
    </keybind>
    <keybind key="W-S-Left">
      <action name="MoveToOutput" direction="left" wrap="yes" />
    </keybind>
    <keybind key="W-S-Right">
      <action name="MoveToOutput" direction="right" wrap="yes" />
    </keybind>
    <keybind key="W-f">
      <action name="Execute" command="tuilnux search --popup" />
    </keybind>
  </keyboard>

  <mouse>
    <doubleClickTime>{double_click}</doubleClickTime>
    <default />
  </mouse>

  <libinput>
    <device category="default">
      <naturalScroll>{natural}</naturalScroll>
      <leftHanded>{left}</leftHanded>
      <pointerSpeed>{speed:.2}</pointerSpeed>
      <accelProfile>{accel}</accelProfile>
      <tap>{tap}</tap>
      <scrollFactor>{scroll:.2}</scrollFactor>
    </device>
  </libinput>

  <windowRules>
    <windowRule identifier="tuilnux-desktop" serverDecoration="no" skipTaskbar="yes"
                skipWindowSwitcher="yes" fixedPosition="yes">
      <action name="ToggleAlwaysOnBottom" />{desktop_output}
    </windowRule>
    <windowRule identifier="tuilnux-search" serverDecoration="no" skipTaskbar="yes"
                skipWindowSwitcher="yes">
      <action name="AutoPlace" policy="center" />
      <action name="ToggleAlwaysOnTop" />
    </windowRule>
  </windowRules>
</labwc_config>
"#,
        gap = g.gap,
        adaptive = yes(s.graphics.adaptive_sync),
        tearing = yes(s.graphics.allow_tearing),
        desktop_output = if s.displays.desktop.is_empty() {
            String::new()
        } else {
            format!(
                "\n      <action name=\"MoveToOutput\" output=\"{}\" />",
                s.displays.desktop
            )
        },
        placement = g.placement.key(),
        layout = g.buttons.labwc(),
        show_title = yes(g.show_title),
        radius = g.corner_radius,
        shadows = yes(g.drop_shadows),
        font = g.title_font,
        font_size = g.title_font_size,
        follow = yes(m.focus_follows_mouse),
        raise = yes(m.raise_on_focus),
        top_max = yes(g.snap_top_maximize),
        numlock = if k.numlock { "on" } else { "off" },
        rate = k.repeat_rate,
        delay = k.repeat_delay,
        double_click = m.double_click_ms,
        natural = yes(m.natural_scroll),
        left = yes(m.left_handed),
        speed = m.speed,
        accel = m.accel.key(),
        tap = yes(m.tap_to_click),
        scroll = m.scroll_factor,
    )
}

/// `themerc-override`: Fensterrahmen, Menüs und Fensterwechsler in den Theme-Farben.
pub fn labwc_theme(s: &Settings) -> String {
    let p = s.palette();
    let inactive_border = hex(mix(p.dim, p.panel, 0.5));
    let (accent, sel, panel) = (hex(p.accent), hex(p.selection), hex(p.panel));
    let (fg, dim, error) = (hex(p.fg), hex(p.dim), hex(p.error));
    format!(
        "# Fensterrahmen in den TUILNUX-Farben (aus tuilnux.toml erzeugt)\n\
         border.width: 1\n\
         window.titlebar.padding.height: 2\n\
         \n\
         window.active.border.color: {accent}\n\
         window.inactive.border.color: {inactive_border}\n\
         \n\
         window.active.title.bg.color: {sel}\n\
         window.inactive.title.bg.color: {panel}\n\
         \n\
         window.active.label.text.color: {accent}\n\
         window.inactive.label.text.color: {dim}\n\
         window.label.text.justify: left\n\
         \n\
         window.active.button.unpressed.image.color: {fg}\n\
         window.inactive.button.unpressed.image.color: {dim}\n\
         window.active.button.close.unpressed.image.color: {error}\n\
         window.button.hover.bg.color: {accent}40\n\
         \n\
         menu.border.color: {accent}\n\
         menu.items.bg.color: {panel}\n\
         menu.items.text.color: {fg}\n\
         menu.items.active.bg.color: {sel}\n\
         menu.items.active.text.color: {accent}\n\
         menu.separator.color: {inactive_border}\n\
         menu.title.bg.color: {sel}\n\
         menu.title.text.color: {accent}\n\
         \n\
         osd.bg.color: {panel}\n\
         osd.border.color: {accent}\n\
         osd.label.text.color: {fg}\n\
         osd.window-switcher.style-classic.item.active.border.color: {accent}\n\
         osd.window-switcher.style-classic.item.active.bg.color: {sel}\n"
    )
}

/// `environment` für labwc: Tastaturlayout und Mauszeiger.
pub fn labwc_environment(s: &Settings) -> String {
    let k = &s.keyboard;
    let g = &s.gui;
    let mut out = String::from("# erzeugt von tsettings\n");
    for (key, value) in [
        ("XKB_DEFAULT_LAYOUT", &k.layout),
        ("XKB_DEFAULT_VARIANT", &k.variant),
        ("XKB_DEFAULT_OPTIONS", &k.options),
        ("XCURSOR_THEME", &g.cursor_theme),
    ] {
        if !value.trim().is_empty() {
            let _ = writeln!(out, "{key}={}", value.trim());
        }
    }
    let _ = writeln!(out, "XCURSOR_SIZE={}", g.cursor_size);
    for (key, value) in s.wayland.env() {
        let _ = writeln!(out, "{key}={value}");
    }
    out
}

/// `~/.config/tuilnux/session.conf` für `tuilnux-session` (Shell-Syntax).
pub fn session_conf(s: &Settings) -> String {
    let t = &s.terminal;
    let terminal = match t.program {
        crate::TerminalProgram::Auto => "",
        other => other.key(),
    };
    let mut out = format!(
        "# erzeugt von tsettings – wird beim nächsten Speichern überschrieben\n\
         TUILNUX_BACKEND={}\n\
         TUILNUX_TERMINAL={terminal}\n\
         TUILNUX_FONT='{}:size={}'\n",
        t.backend.key(),
        t.font,
        t.font_size
    );
    let graphics = s.graphics.exports();
    if !graphics.is_empty() {
        out.push_str("\n# Grafik (tsettings → Grafik)\n");
        out.push_str(&graphics);
    }
    out
}

/// Schlüssel für `gtk-3.0/settings.ini` und `gtk-4.0/settings.ini`.
pub fn gtk_settings(s: &Settings) -> Vec<(&'static str, String)> {
    let g = &s.gui;
    let mut out = vec![
        ("gtk-decoration-layout", g.buttons.gtk().to_string()),
        (
            "gtk-application-prefer-dark-theme",
            if g.prefer_dark { "1" } else { "0" }.to_string(),
        ),
        ("gtk-cursor-theme-size", g.cursor_size.to_string()),
        ("gtk-double-click-time", s.mouse.double_click_ms.to_string()),
    ];
    if !g.cursor_theme.trim().is_empty() {
        out.push(("gtk-cursor-theme-name", g.cursor_theme.trim().to_string()));
    }
    out
}

/// Setzt `pairs` im Abschnitt `[section]` einer INI-Datei und lässt alles andere stehen.
pub fn merge_ini(existing: &str, section: &str, pairs: &[(&str, String)]) -> String {
    let header = format!("[{section}]");
    let mut lines: Vec<String> = existing.lines().map(str::to_string).collect();
    let start = lines.iter().position(|l| l.trim() == header);
    let start = match start {
        Some(i) => i,
        None => {
            if lines.last().is_some_and(|l| !l.trim().is_empty()) {
                lines.push(String::new());
            }
            lines.push(header);
            lines.len() - 1
        }
    };
    let end = lines[start + 1..]
        .iter()
        .position(|l| l.trim_start().starts_with('['))
        .map_or(lines.len(), |p| start + 1 + p);
    let mut missing = Vec::new();
    for (key, value) in pairs {
        let found = (start + 1..end).find(|&i| {
            lines[i]
                .split_once('=')
                .is_some_and(|(k, _)| k.trim() == *key)
        });
        match found {
            Some(i) => lines[i] = format!("{key}={value}"),
            None => missing.push(format!("{key}={value}")),
        }
    }
    // Neue Schlüssel ans Ende des Abschnitts (vor Leerzeilen).
    let mut at = end;
    while at > start + 1 && lines[at - 1].trim().is_empty() {
        at -= 1;
    }
    for (n, line) in missing.into_iter().enumerate() {
        lines.insert(at + n, line);
    }
    let mut out = lines.join("\n");
    out.push('\n');
    out
}

/// Ergebnis von [`apply`].
#[derive(Debug, Default)]
pub struct Applied {
    pub written: Vec<PathBuf>,
    /// Hinweise für den Benutzer (z. B. „wirkt nach erneutem Anmelden“).
    pub notes: Vec<String>,
}

fn write(path: &Path, content: &str, applied: &mut Applied) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).with_context(|| dir.display().to_string())?;
    }
    if std::fs::read_to_string(path).is_ok_and(|old| old == content) {
        return Ok(());
    }
    write_atomic(path, content)?;
    applied.written.push(path.to_path_buf());
    Ok(())
}

fn have(program: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|paths| std::env::split_paths(&paths).any(|dir| dir.join(program).is_file()))
}

fn quiet(cmd: &mut Command) -> bool {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// Bringt eine vorhandene, von tsettings erzeugte labwc-Konfiguration auf den
/// Stand dieser Version (z. B. neue Tastenkürzel nach einem Update) und lädt
/// labwc neu. Gibt zurück, ob sich etwas geändert hat.
pub fn refresh_labwc(s: &Settings) -> Result<bool> {
    let Some(dir) = config_dir() else {
        return Ok(false);
    };
    let labwc = dir.join("labwc");
    if !labwc.join("rc.xml").is_file() {
        return Ok(false);
    }
    let mut applied = Applied::default();
    write(&labwc.join("rc.xml"), &labwc_rc(s), &mut applied)?;
    write(
        &labwc.join("themerc-override"),
        &labwc_theme(s),
        &mut applied,
    )?;
    if applied.written.is_empty() {
        return Ok(false);
    }
    if std::env::var_os("LABWC_PID").is_some() && have("labwc") {
        quiet(Command::new("labwc").arg("--reconfigure"));
    }
    Ok(true)
}

/// Speichert die Einstellungen und überträgt sie auf labwc, GTK und die Sitzung.
/// Die laufende tuilnux-Sitzung lädt ihre Werte per IPC (`tuilnux reload`) neu.
pub fn apply(s: &Settings, previous: Option<&Settings>) -> Result<Applied> {
    let mut applied = Applied::default();
    let file = s.save()?;
    applied.written.push(file);
    let dir = config_dir().context("kein Konfigurationsverzeichnis gefunden")?;

    let labwc = dir.join("labwc");
    write(&labwc.join("rc.xml"), &labwc_rc(s), &mut applied)?;
    write(
        &labwc.join("themerc-override"),
        &labwc_theme(s),
        &mut applied,
    )?;
    write(
        &labwc.join("environment"),
        &labwc_environment(s),
        &mut applied,
    )?;
    write(&dir.join("session.conf"), &session_conf(s), &mut applied)?;

    let gtk = gtk_settings(s);
    if let Some(home_config) = dirs::config_dir() {
        for version in ["gtk-3.0", "gtk-4.0"] {
            let path = home_config.join(version).join("settings.ini");
            let old = std::fs::read_to_string(&path).unwrap_or_default();
            write(&path, &merge_ini(&old, "Settings", &gtk), &mut applied)?;
        }
    }

    // GTK unter Wayland liest diese Werte bevorzugt aus GSettings.
    if have("gsettings") {
        let g = &s.gui;
        let set = |schema: &str, key: &str, value: String| {
            quiet(Command::new("gsettings").args(["set", schema, key, &value]));
        };
        set(
            "org.gnome.desktop.wm.preferences",
            "button-layout",
            g.buttons.gtk().replace("menu", "appmenu"),
        );
        set(
            "org.gnome.desktop.interface",
            "color-scheme",
            if g.prefer_dark {
                "prefer-dark"
            } else {
                "default"
            }
            .into(),
        );
        set(
            "org.gnome.desktop.interface",
            "cursor-size",
            g.cursor_size.to_string(),
        );
        if !g.cursor_theme.trim().is_empty() {
            set(
                "org.gnome.desktop.interface",
                "cursor-theme",
                g.cursor_theme.trim().to_string(),
            );
        }
    }

    if std::env::var_os("LABWC_PID").is_some() && have("labwc") {
        if quiet(Command::new("labwc").arg("--reconfigure")) {
            applied
                .notes
                .push("labwc neu geladen (Fenster, Tastatur, Maus)".into());
        }
    } else {
        applied
            .notes
            .push("labwc läuft nicht – Fenster-, Tastatur- und Mauswerte wirken ab der nächsten labwc-Sitzung".into());
    }
    let displays_changed = previous.is_none_or(|p| p.displays != s.displays);
    if displays_changed && std::env::var_os("WAYLAND_DISPLAY").is_some() {
        match crate::apply_outputs(&s.displays, true) {
            Ok(Some(note)) => applied.notes.push(note),
            Ok(None) => {}
            Err(e) => applied.notes.push(format!("Bildschirme: {e:#}")),
        }
    }
    if let Some(prev) = previous {
        if prev.terminal != s.terminal {
            applied
                .notes
                .push("Terminal, Schrift und Backend wirken nach erneutem Anmelden".into());
        }
        if prev.gui.cursor_theme != s.gui.cursor_theme
            || prev.gui.cursor_size != s.gui.cursor_size
            || prev.gui.prefer_dark != s.gui.prefer_dark
            || prev.gui.buttons != s.gui.buttons
        {
            applied.notes.push(
                "GTK-Programme (Firefox …) übernehmen Knöpfe, Zeiger und Farbschema beim nächsten Start"
                    .into(),
            );
        }
        if prev.graphics != s.graphics || prev.wayland != s.wayland {
            applied
                .notes
                .push("Grafik- und Wayland-Einstellungen wirken nach erneutem Anmelden".into());
        }
        if prev.displays.desktop != s.displays.desktop {
            applied
                .notes
                .push("Desktop-Bildschirm wechselt nach erneutem Anmelden".into());
        }
        if prev.bash != s.bash {
            applied
                .notes
                .push("Shell-Einstellungen gelten in neuen Terminals (oder: exec bash)".into());
        }
        if prev.shell != s.shell || prev.autostart != s.autostart {
            applied
                .notes
                .push("Shell und Autostart wirken nach erneutem Anmelden".into());
        }
    }
    Ok(applied)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{GuiPlacement, ThemePreset, TitleButtons};

    fn dist(name: &str) -> String {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../dist")
            .join(name);
        std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("{}: {e}", path.display()))
            .replace("\r\n", "\n")
    }

    /// Die mitgelieferten Standarddateien entsprechen den Standardeinstellungen.
    /// Neu erzeugen: `TUILNUX_REGENERATE=1 cargo test -p tuilnux-config`.
    #[test]
    fn dist_files_match_defaults() {
        let s = Settings::default();
        if std::env::var_os("TUILNUX_REGENERATE").is_some() {
            let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../dist");
            std::fs::create_dir_all(root.join("gtk")).unwrap();
            std::fs::write(root.join("labwc/rc.xml"), labwc_rc(&s)).unwrap();
            std::fs::write(root.join("labwc/themerc-override"), labwc_theme(&s)).unwrap();
            let ini = merge_ini("", "Settings", &gtk_settings(&s));
            std::fs::write(root.join("gtk/settings.ini"), ini).unwrap();
        }
        assert_eq!(
            dist("labwc/rc.xml"),
            labwc_rc(&s),
            "dist/labwc/rc.xml neu erzeugen"
        );
        assert_eq!(dist("labwc/themerc-override"), labwc_theme(&s));
        let ini = merge_ini("", "Settings", &gtk_settings(&s));
        assert_eq!(dist("gtk/settings.ini"), ini);
    }

    #[test]
    fn labwc_reflects_settings() {
        let mut s = Settings::default();
        s.gui.buttons = TitleButtons::Left;
        s.gui.placement = GuiPlacement::Center;
        s.mouse.natural_scroll = true;
        s.mouse.speed = -0.5;
        s.keyboard.repeat_rate = 40;
        s.keyboard.layout = "de,us".into();
        s.appearance.theme = ThemePreset::Dracula;
        let rc = labwc_rc(&s);
        assert!(rc.contains("<layout>close,iconify,max:icon</layout>"));
        assert!(rc.contains("<policy>center</policy>"));
        assert!(rc.contains("<naturalScroll>yes</naturalScroll>"));
        assert!(rc.contains("<pointerSpeed>-0.50</pointerSpeed>"));
        assert!(rc.contains("<repeatRate>40</repeatRate>"));
        assert!(labwc_theme(&s).contains("window.active.border.color: #bd93f9"));
        let env = labwc_environment(&s);
        assert!(env.contains("XKB_DEFAULT_LAYOUT=de,us\n"));
        assert!(!env.contains("XKB_DEFAULT_VARIANT"));
        assert!(session_conf(&s).contains("TUILNUX_FONT='monospace:size=10'"));
    }

    #[test]
    fn ini_merge_keeps_other_values() {
        let old = "# Kommentar\n[Settings]\ngtk-theme-name=Adwaita\ngtk-decoration-layout=menu:close\n\n[Other]\na=1\n";
        let pairs = [
            (
                "gtk-decoration-layout",
                "menu:minimize,maximize,close".to_string(),
            ),
            ("gtk-cursor-theme-size", "32".to_string()),
        ];
        let new = merge_ini(old, "Settings", &pairs);
        assert_eq!(
            new,
            "# Kommentar\n[Settings]\ngtk-theme-name=Adwaita\ngtk-decoration-layout=menu:minimize,maximize,close\ngtk-cursor-theme-size=32\n\n[Other]\na=1\n"
        );
        assert_eq!(merge_ini(&new, "Settings", &pairs), new);
        assert_eq!(
            merge_ini("", "Settings", &pairs[..1]),
            "[Settings]\ngtk-decoration-layout=menu:minimize,maximize,close\n"
        );
    }
}
