//! Bildschirme: Auflösung, Skalierung, Drehung, Anordnung und der Monitor des
//! Desktops. Erkannt und angewandt über `wlr-randr` (wlr-output-management),
//! das unter labwc läuft.

use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::Choice;

choice!(
    /// Wie mehrere Bildschirme nebeneinander liegen.
    Arrangement [Right] {
        Right = "right", "nebeneinander (links → rechts)";
        Left = "left", "nebeneinander (rechts → links)";
        Below = "below", "untereinander";
        Manual = "manual", "manuell (Position je Monitor)";
    }
);

choice!(
    /// Drehung eines Bildschirms.
    Transform [Normal] {
        Normal = "normal", "normal";
        R90 = "90", "90° (Hochformat)";
        R180 = "180", "180° (kopfüber)";
        R270 = "270", "270° (Hochformat links)";
        Flipped = "flipped", "gespiegelt";
    }
);

impl Transform {
    /// Breite und Höhe tauschen?
    pub fn swaps(self) -> bool {
        matches!(self, Self::R90 | Self::R270)
    }
}

/// Einstellungen eines Monitors (per Name, z. B. `HDMI-A-1`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct OutputSettings {
    pub name: String,
    pub enabled: bool,
    /// `BREITExHÖHE` oder `BREITExHÖHE@HZ`; leer = bevorzugter Modus.
    pub mode: String,
    pub scale: f32,
    pub transform: Transform,
    /// Position bei [`Arrangement::Manual`] (logische Pixel).
    pub x: i32,
    pub y: i32,
    /// Variable Bildwiederholrate (FreeSync/G-Sync), falls unterstützt.
    pub adaptive_sync: bool,
}

impl Default for OutputSettings {
    fn default() -> Self {
        Self {
            name: String::new(),
            enabled: true,
            mode: String::new(),
            scale: 1.0,
            transform: Transform::Normal,
            x: 0,
            y: 0,
            adaptive_sync: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DisplaySettings {
    /// Monitor des Desktop-Terminals und der Statusleiste (leer = automatisch).
    pub desktop: String,
    pub arrangement: Arrangement,
    /// Ohne festen Modus: empfohlene Auflösung mit der höchsten Bildrate statt
    /// des (oft auf 60 Hz stehenden) bevorzugten Modus.
    pub max_refresh: bool,
    pub outputs: Vec<OutputSettings>,
}

impl Default for DisplaySettings {
    fn default() -> Self {
        Self {
            desktop: String::new(),
            arrangement: Arrangement::default(),
            max_refresh: true,
            outputs: Vec::new(),
        }
    }
}

/// Erlaubte Zeichen in Monitornamen (gehen in rc.xml und an wlr-randr).
pub fn output_name_ok(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_.:".contains(c))
}

/// `1920x1080` oder `1920x1080@60` / `@59.94`.
pub fn parse_mode(mode: &str) -> Option<(u32, u32, Option<f32>)> {
    let (size, refresh) = match mode.split_once('@') {
        Some((s, r)) => (s, Some(r.trim_end_matches("Hz").parse::<f32>().ok()?)),
        None => (mode, None),
    };
    let (w, h) = size.split_once('x')?;
    let (w, h) = (w.parse().ok()?, h.parse().ok()?);
    (w > 0 && h > 0 && refresh.is_none_or(|r| r > 0.0)).then_some((w, h, refresh))
}

impl DisplaySettings {
    pub fn validate(&self) -> Result<()> {
        if !self.desktop.is_empty() && !output_name_ok(&self.desktop) {
            bail!(
                "displays.desktop = \"{}\" ist kein Monitorname",
                self.desktop
            );
        }
        for o in &self.outputs {
            if !output_name_ok(&o.name) {
                bail!("Monitorname „{}“ ist ungültig", o.name);
            }
            if !o.mode.is_empty() && parse_mode(&o.mode).is_none() {
                bail!(
                    "Auflösung „{}“ von {}: erwartet BREITExHÖHE oder BREITExHÖHE@HZ",
                    o.mode,
                    o.name
                );
            }
            if !(0.25..=4.0).contains(&o.scale) {
                bail!("Skalierung von {} muss zwischen 0.25 und 4 liegen", o.name);
            }
        }
        Ok(())
    }

    pub fn output(&self, name: &str) -> Option<&OutputSettings> {
        self.outputs.iter().find(|o| o.name == name)
    }

    /// Eintrag für `name`, bei Bedarf mit Standardwerten angelegt.
    pub fn output_mut(&mut self, name: &str) -> &mut OutputSettings {
        let i = match self.outputs.iter().position(|o| o.name == name) {
            Some(i) => i,
            None => {
                self.outputs.push(OutputSettings {
                    name: name.to_string(),
                    ..OutputSettings::default()
                });
                self.outputs.len() - 1
            }
        };
        &mut self.outputs[i]
    }
}

/// Ein Modus, wie ihn der Monitor meldet.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ModeInfo {
    pub width: u32,
    pub height: u32,
    /// Hz
    pub refresh: f32,
    #[serde(default)]
    pub preferred: bool,
    #[serde(default)]
    pub current: bool,
}

impl ModeInfo {
    /// Schlüssel für `OutputSettings::mode`.
    pub fn key(&self) -> String {
        format!("{}x{}@{:.3}", self.width, self.height, self.refresh)
    }

    pub fn label(&self) -> String {
        let mut s = format!("{}×{} @ {:.2} Hz", self.width, self.height, self.refresh);
        if self.preferred {
            s.push_str(" (empfohlen)");
        }
        s
    }
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct Position {
    pub x: i32,
    pub y: i32,
}

/// Ein angeschlossener Monitor (Ausgabe von `wlr-randr --json`).
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct OutputInfo {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub make: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub modes: Vec<ModeInfo>,
    #[serde(default)]
    pub position: Option<Position>,
    #[serde(default)]
    pub transform: Option<String>,
    #[serde(default)]
    pub scale: Option<f32>,
    #[serde(default)]
    pub adaptive_sync: Option<bool>,
}

impl OutputInfo {
    pub fn current(&self) -> Option<&ModeInfo> {
        self.modes.iter().find(|m| m.current)
    }

    pub fn preferred(&self) -> Option<&ModeInfo> {
        self.modes
            .iter()
            .find(|m| m.preferred)
            .or(self.modes.first())
    }

    /// Automatische Wahl: empfohlene Auflösung mit der höchsten Bildrate.
    pub fn best_mode(&self) -> Option<&ModeInfo> {
        let base = self.preferred()?;
        self.modes
            .iter()
            .filter(|m| m.width == base.width && m.height == base.height)
            .max_by(|a, b| a.refresh.total_cmp(&b.refresh))
    }

    /// Passender Modus zu einer Einstellung; `144` findet auch `143.981`.
    pub fn find_mode(&self, w: u32, h: u32, refresh: Option<f32>) -> Option<&ModeInfo> {
        self.modes
            .iter()
            .filter(|m| m.width == w && m.height == h)
            .filter(|m| refresh.is_none_or(|r| (m.refresh - r).abs() < 1.0))
            .min_by(|a, b| {
                let r = refresh.unwrap_or(f32::MAX);
                (a.refresh - r).abs().total_cmp(&(b.refresh - r).abs())
            })
    }

    /// Bester Modus, falls er nicht schon läuft.
    fn upgrade(&self) -> Option<&ModeInfo> {
        let best = self.best_mode()?;
        let same = self.current().is_some_and(|c| c.key() == best.key());
        (!same).then_some(best)
    }

    /// Hersteller und Modell, sonst der Name.
    pub fn product(&self) -> String {
        let parts: Vec<&str> = [self.make.as_deref(), self.model.as_deref()]
            .into_iter()
            .flatten()
            .filter(|s| !s.is_empty() && *s != "(null)")
            .collect();
        if parts.is_empty() {
            self.name.clone()
        } else {
            parts.join(" ")
        }
    }

    /// Kurzbeschreibung für tsettings.
    pub fn summary(&self) -> String {
        let mode = self.current().map_or("aus".into(), |m| {
            format!("{}×{} @ {:.0} Hz", m.width, m.height, m.refresh)
        });
        let pos = self
            .position
            .as_ref()
            .map_or(String::new(), |p| format!(" bei {},{}", p.x, p.y));
        let scale = self
            .scale
            .filter(|s| (*s - 1.0).abs() > 0.01)
            .map_or(String::new(), |s| format!(" ×{s}"));
        format!("{} · {mode}{scale}{pos}", self.product())
    }

    /// Modi ohne Doppelte (gleiche Größe und Rate).
    pub fn unique_modes(&self) -> Vec<&ModeInfo> {
        let mut out: Vec<&ModeInfo> = Vec::new();
        for m in &self.modes {
            if !out.iter().any(|o| o.key() == m.key()) {
                out.push(m);
            }
        }
        out
    }
}

pub fn parse_wlr_randr(json: &str) -> Result<Vec<OutputInfo>> {
    serde_json::from_str(json).context("Ausgabe von wlr-randr --json nicht lesbar")
}

/// Fragt die angeschlossenen Monitore ab (braucht `wlr-randr` und Wayland).
pub fn detect_outputs() -> Result<Vec<OutputInfo>> {
    if std::env::var_os("WAYLAND_DISPLAY").is_none() {
        bail!("keine Wayland-Sitzung");
    }
    let out = Command::new("wlr-randr")
        .arg("--json")
        .stdin(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .context("wlr-randr fehlt (sudo pacman -S wlr-randr)")?;
    if !out.status.success() {
        bail!("wlr-randr: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    parse_wlr_randr(&String::from_utf8_lossy(&out.stdout))
}

/// Logische Größe eines Monitors nach Modus, Skalierung und Drehung.
fn logical_size(info: &OutputInfo, cfg: Option<&OutputSettings>) -> (i32, i32) {
    let mode = cfg
        .and_then(|c| parse_mode(&c.mode))
        .map(|(w, h, _)| (w, h))
        .or_else(|| info.preferred().map(|m| (m.width, m.height)))
        .or_else(|| info.current().map(|m| (m.width, m.height)))
        .unwrap_or((1920, 1080));
    let scale = cfg.map_or(info.scale.unwrap_or(1.0), |c| c.scale).max(0.25);
    let (w, h) = if cfg.is_some_and(|c| c.transform.swaps()) {
        (mode.1, mode.0)
    } else {
        mode
    };
    (
        (w as f32 / scale).round() as i32,
        (h as f32 / scale).round() as i32,
    )
}

/// Argumente für wlr-randr, die `settings` auf die erkannten Monitore anwenden.
/// Leer, wenn nichts zu tun ist (ein Monitor ohne eigene Einstellungen).
/// Mit `force` werden Monitore ohne eigene Einstellungen auf die Standardwerte
/// (bevorzugte Auflösung, Skalierung 1, keine Drehung) zurückgesetzt.
pub fn wlr_randr_args(
    settings: &DisplaySettings,
    outputs: &[OutputInfo],
    force: bool,
) -> Vec<String> {
    let upgrade = |info: &OutputInfo| {
        settings
            .max_refresh
            .then(|| info.upgrade())
            .flatten()
            .map(|m| format!("{}x{}@{:.3}Hz", m.width, m.height, m.refresh))
    };
    if !force
        && outputs.len() < 2
        && settings.outputs.is_empty()
        && outputs.iter().all(|o| upgrade(o).is_none())
    {
        return Vec::new();
    }
    let fallback = OutputSettings::default();
    let mut args = Vec::new();
    let mut offset = 0i32;
    let order: Vec<&OutputInfo> = match settings.arrangement {
        Arrangement::Left => outputs.iter().rev().collect(),
        _ => outputs.iter().collect(),
    };
    for info in order {
        let cfg = settings.output(&info.name).or(force.then_some(&fallback));
        args.push("--output".to_string());
        args.push(info.name.clone());
        if cfg.is_some_and(|c| !c.enabled) {
            args.push("--off".into());
            continue;
        }
        args.push("--on".into());
        if let Some(c) = cfg {
            if let Some((w, h, refresh)) = parse_mode(&c.mode) {
                args.push("--mode".into());
                args.push(match (info.find_mode(w, h, refresh), refresh) {
                    (Some(m), Some(_)) => format!("{w}x{h}@{:.3}Hz", m.refresh),
                    (_, Some(r)) => format!("{w}x{h}@{r}Hz"),
                    (_, None) => format!("{w}x{h}"),
                });
            } else if let Some(best) = settings.max_refresh.then(|| info.best_mode()).flatten() {
                args.push("--mode".into());
                args.push(format!(
                    "{}x{}@{:.3}Hz",
                    best.width, best.height, best.refresh
                ));
            } else if info.modes.iter().any(|m| m.preferred) {
                args.push("--preferred".into());
            }
            args.push("--scale".into());
            args.push(format!("{}", c.scale));
            args.push("--transform".into());
            args.push(c.transform.key().into());
            if info.adaptive_sync.is_some() {
                args.push("--adaptive-sync".into());
                args.push(
                    if c.adaptive_sync {
                        "enabled"
                    } else {
                        "disabled"
                    }
                    .into(),
                );
            }
        } else if let Some(mode) = upgrade(info) {
            args.push("--mode".into());
            args.push(mode);
        }
        let (w, h) = logical_size(info, cfg);
        let (x, y) = match settings.arrangement {
            Arrangement::Manual => cfg.map_or((0, 0), |c| (c.x, c.y)),
            Arrangement::Below => (0, offset),
            Arrangement::Right | Arrangement::Left => (offset, 0),
        };
        offset += if settings.arrangement == Arrangement::Below {
            h
        } else {
            w
        };
        args.push("--pos".into());
        args.push(format!("{x},{y}"));
    }
    args
}

/// Wendet die Bildschirm-Einstellungen an. Gibt eine Meldung zurück, wenn etwas
/// geändert wurde; ohne Wayland/wlr-randr passiert nichts. `force` setzt auch
/// Monitore ohne eigene Einstellungen zurück (z. B. nach „Standard“).
pub fn apply_outputs(settings: &DisplaySettings, force: bool) -> Result<Option<String>> {
    let outputs = detect_outputs()?;
    let args = wlr_randr_args(settings, &outputs, force);
    if args.is_empty() {
        return Ok(None);
    }
    let out = Command::new("wlr-randr")
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .context("wlr-randr")?;
    if !out.status.success() {
        bail!(
            "Bildschirme: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    let n = outputs.len();
    Ok(Some(if n == 1 {
        "Bildschirm eingestellt".into()
    } else {
        format!("{n} Bildschirme eingestellt")
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    const JSON: &str = r#"[
      {"name":"Virtual-1","description":"(null) (null) (Virtual-1)","make":null,"model":null,
       "serial":null,"physical_size":{"width":0,"height":0},"enabled":true,
       "modes":[{"width":1280,"height":800,"refresh":60.000000,"preferred":true,"current":true},
                {"width":1920,"height":1080,"refresh":60.000000,"preferred":false,"current":false},
                {"width":1920,"height":1080,"refresh":60.000000,"preferred":false,"current":false}],
       "position":{"x":0,"y":0},"transform":"normal","scale":1.000000,"adaptive_sync":false},
      {"name":"HDMI-A-1","make":"Dell","model":"U2720Q","enabled":true,
       "modes":[{"width":3840,"height":2160,"refresh":59.997,"preferred":true,"current":true}],
       "position":{"x":1280,"y":0},"transform":"normal","scale":2.0}
    ]"#;

    #[test]
    fn parses_wlr_randr() {
        let outs = parse_wlr_randr(JSON).unwrap();
        assert_eq!(outs.len(), 2);
        assert_eq!(outs[0].product(), "Virtual-1");
        assert_eq!(outs[1].product(), "Dell U2720Q");
        assert_eq!(outs[0].unique_modes().len(), 2);
        assert_eq!(outs[0].current().unwrap().key(), "1280x800@60.000");
        assert!(outs[1].summary().contains("3840×2160"));
    }

    #[test]
    fn modes() {
        assert_eq!(parse_mode("1920x1080"), Some((1920, 1080, None)));
        assert_eq!(
            parse_mode("1920x1080@59.94"),
            Some((1920, 1080, Some(59.94)))
        );
        assert_eq!(parse_mode("1920x1080@60Hz"), Some((1920, 1080, Some(60.0))));
        assert_eq!(parse_mode("1920"), None);
        assert_eq!(parse_mode("0x1080"), None);
    }

    #[test]
    fn arranges_outputs() {
        let outs = parse_wlr_randr(JSON).unwrap();
        let classic = DisplaySettings {
            max_refresh: false,
            ..Default::default()
        };
        let mut s = classic.clone();
        let args = wlr_randr_args(&s, &outs, false).join(" ");
        assert_eq!(
            args,
            "--output Virtual-1 --on --pos 0,0 --output HDMI-A-1 --on --pos 1280,0"
        );
        s.output_mut("HDMI-A-1").scale = 1.5;
        s.output_mut("Virtual-1").mode = "1920x1080@60.000".into();
        let args = wlr_randr_args(&s, &outs, false).join(" ");
        assert!(args.contains("--output Virtual-1 --on --mode 1920x1080@60.000Hz --scale 1 --transform normal --adaptive-sync disabled --pos 0,0"));
        assert!(args.contains(
            "--output HDMI-A-1 --on --preferred --scale 1.5 --transform normal --pos 1920,0"
        ));
        s.arrangement = Arrangement::Below;
        s.output_mut("Virtual-1").transform = Transform::R90;
        let args = wlr_randr_args(&s, &outs, false).join(" ");
        assert!(args.ends_with("--pos 0,1920"), "{args}");
        s.output_mut("HDMI-A-1").enabled = false;
        assert!(
            wlr_randr_args(&s, &outs, false)
                .join(" ")
                .contains("HDMI-A-1 --off")
        );

        // Ein Monitor ohne Einstellungen: nichts tun.
        assert!(wlr_randr_args(&classic, &outs[..1], false).is_empty());
        // Explizit angewendet: auf Standardwerte zurücksetzen.
        assert_eq!(
            wlr_randr_args(&classic, &outs[..1], true).join(" "),
            "--output Virtual-1 --on --preferred --scale 1 --transform normal --adaptive-sync disabled --pos 0,0"
        );
        // Ohne bevorzugten Modus (z. B. headless) kein --preferred.
        let mut plain = outs[..1].to_vec();
        plain[0].modes.iter_mut().for_each(|m| m.preferred = false);
        assert!(
            !wlr_randr_args(&classic, &plain, true)
                .join(" ")
                .contains("--preferred")
        );
    }

    #[test]
    fn picks_highest_refresh_automatically() {
        let mut outs = parse_wlr_randr(JSON).unwrap();
        outs.truncate(1);
        let auto = DisplaySettings::default();
        assert!(auto.max_refresh);
        // Läuft schon im besten Modus: nichts zu tun.
        assert!(wlr_randr_args(&auto, &outs, false).is_empty());
        outs[0].modes.push(ModeInfo {
            width: 1280,
            height: 800,
            refresh: 143.981,
            preferred: false,
            current: false,
        });
        outs[0].modes.push(ModeInfo {
            width: 1920,
            height: 1080,
            refresh: 240.0,
            preferred: false,
            current: false,
        });
        assert_eq!(outs[0].best_mode().unwrap().refresh, 143.981);
        assert_eq!(
            wlr_randr_args(&auto, &outs, false).join(" "),
            "--output Virtual-1 --on --mode 1280x800@143.981Hz --pos 0,0"
        );
        // Fester Modus hat Vorrang.
        let mut fixed = auto.clone();
        fixed.output_mut("Virtual-1").mode = "1920x1080@240".into();
        assert!(
            wlr_randr_args(&fixed, &outs, false)
                .join(" ")
                .contains("--mode 1920x1080@240.000Hz")
        );
        // Gerundete Hz (z. B. aus dem Installer) treffen den echten Modus.
        fixed.output_mut("Virtual-1").mode = "1280x800@144".into();
        assert!(
            wlr_randr_args(&fixed, &outs, false)
                .join(" ")
                .contains("--mode 1280x800@143.981Hz")
        );
        // Ohne max_refresh bleibt es beim bevorzugten Modus.
        let off = DisplaySettings {
            max_refresh: false,
            ..Default::default()
        };
        assert!(wlr_randr_args(&off, &outs, false).is_empty());
    }

    #[test]
    fn validation() {
        let mut s = DisplaySettings::default();
        assert!(s.validate().is_ok());
        s.desktop = "HDMI<1".into();
        assert!(s.validate().is_err());
        s.desktop = "HDMI-A-1".into();
        s.output_mut("DP-1").mode = "groß".into();
        assert!(s.validate().is_err());
        s.output_mut("DP-1").mode = "2560x1440@144".into();
        s.output_mut("DP-1").scale = 9.0;
        assert!(s.validate().is_err());
    }
}
