//! Bildschirm-Erkennung ohne laufendes Wayland: liest `/sys/class/drm`
//! (Status, Modusliste, EDID) und berechnet daraus Auflösungen samt Bildrate.

use std::path::Path;

#[derive(Debug, Clone, PartialEq)]
pub struct Mode {
    pub width: u32,
    pub height: u32,
    /// Bildrate in Hz; `None`, wenn nur die Auflösung bekannt ist.
    pub refresh: Option<f32>,
}

impl Mode {
    /// Schlüssel für `displays.outputs[].mode`, z. B. `1920x1080@144`.
    pub fn key(&self) -> String {
        match self.refresh {
            Some(r) => format!("{}x{}@{}", self.width, self.height, r.round() as u32),
            None => format!("{}x{}", self.width, self.height),
        }
    }

    pub fn label(&self) -> String {
        match self.refresh {
            Some(r) => format!(
                "{} × {}  @ {} Hz",
                self.width,
                self.height,
                r.round() as u32
            ),
            None => format!("{} × {}", self.width, self.height),
        }
    }

    fn same(&self, other: &Mode) -> bool {
        self.width == other.width
            && self.height == other.height
            && match (self.refresh, other.refresh) {
                (Some(a), Some(b)) => (a - b).abs() < 0.5,
                _ => self.refresh.is_none() && other.refresh.is_none(),
            }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Output {
    /// Name wie bei Wayland/wlr-randr, z. B. `HDMI-A-1`.
    pub name: String,
    pub model: String,
    /// Bevorzugter Modus zuerst, danach absteigend nach Fläche und Bildrate.
    pub modes: Vec<Mode>,
}

impl Output {
    /// Was „automatisch“ wählt: größte Auflösung mit der höchsten Bildrate.
    pub fn best(&self) -> Option<Mode> {
        let preferred = self.modes.first()?;
        let max = self
            .modes
            .iter()
            .filter(|m| m.width == preferred.width && m.height == preferred.height)
            .filter_map(|m| m.refresh)
            .fold(None, |acc: Option<f32>, r| {
                Some(acc.map_or(r, |a| a.max(r)))
            });
        Some(Mode {
            refresh: max,
            ..preferred.clone()
        })
    }
}

/// Alle angeschlossenen Bildschirme.
pub fn detect() -> Vec<Output> {
    detect_in(Path::new("/sys/class/drm"))
}

pub fn detect_in(root: &Path) -> Vec<Output> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut outputs: Vec<Output> = entries
        .flatten()
        .filter_map(|e| {
            let file = e.file_name().to_string_lossy().into_owned();
            // card0-HDMI-A-1 → HDMI-A-1 (Render-Knoten und Karten selbst haben keinen Bindestrich-Anschluss)
            let (card, name) = file.split_once('-')?;
            if !card.starts_with("card") {
                return None;
            }
            let dir = e.path();
            let status = std::fs::read_to_string(dir.join("status")).unwrap_or_default();
            if status.trim() != "connected" {
                return None;
            }
            let listed = std::fs::read_to_string(dir.join("modes")).unwrap_or_default();
            let edid = std::fs::read(dir.join("edid")).unwrap_or_default();
            Some(build(name, &listed, &edid))
        })
        .collect();
    outputs.sort_by(|a, b| a.name.cmp(&b.name));
    outputs
}

fn build(name: &str, listed: &str, edid: &[u8]) -> Output {
    let parsed = parse_edid(edid);
    let mut modes: Vec<Mode> = Vec::new();
    let mut push = |m: Mode| {
        if m.width >= 320 && m.height >= 200 && !modes.iter().any(|o| o.same(&m)) {
            modes.push(m);
        }
    };
    // Der Kernel listet den bevorzugten Modus zuerst.
    let resolutions: Vec<(u32, u32)> = listed.lines().filter_map(parse_resolution).collect();
    let preferred = parsed
        .as_ref()
        .and_then(|e| e.timings.first().map(|m| (m.width, m.height)))
        .or_else(|| resolutions.first().copied());
    let mut timed: Vec<Mode> = parsed
        .as_ref()
        .map(|e| e.timings.clone())
        .unwrap_or_default();
    timed.sort_by(|a, b| {
        (b.width * b.height).cmp(&(a.width * a.height)).then(
            b.refresh
                .unwrap_or(0.0)
                .total_cmp(&a.refresh.unwrap_or(0.0)),
        )
    });
    if let Some((w, h)) = preferred {
        for m in timed.iter().filter(|m| m.width == w && m.height == h) {
            push(m.clone());
        }
        if !timed.iter().any(|m| m.width == w && m.height == h) {
            push(Mode {
                width: w,
                height: h,
                refresh: None,
            });
        }
    }
    for m in &timed {
        push(m.clone());
    }
    for (w, h) in resolutions {
        if !timed.iter().any(|m| m.width == w && m.height == h) {
            push(Mode {
                width: w,
                height: h,
                refresh: None,
            });
        }
    }
    Output {
        name: name.to_string(),
        model: parsed.map(|e| e.name).unwrap_or_default(),
        modes,
    }
}

/// Zeile aus `/sys/class/drm/*/modes`, z. B. `1920x1080` (Zeilensprung `i` wird übersprungen).
fn parse_resolution(line: &str) -> Option<(u32, u32)> {
    let (w, h) = line.trim().split_once('x')?;
    Some((w.parse().ok()?, h.parse().ok()?))
}

#[derive(Debug, Default)]
pub struct Edid {
    pub name: String,
    /// Detaillierte Timings zuerst (das erste ist der bevorzugte Modus), dann Standard-Timings.
    pub timings: Vec<Mode>,
}

const EDID_HEADER: [u8; 8] = [0, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0];

pub fn parse_edid(data: &[u8]) -> Option<Edid> {
    if data.len() < 128 || data[..8] != EDID_HEADER {
        return None;
    }
    let mut edid = Edid::default();
    for offset in [54, 72, 90, 108] {
        descriptor(&data[offset..offset + 18], &mut edid);
    }
    for pair in data[38..54].chunks(2) {
        if let Some(m) = standard_timing(pair[0], pair[1], data[19]) {
            edid.timings.push(m);
        }
    }
    // CTA-861-Erweiterungen (HDMI/DisplayPort) enthalten weitere detaillierte Timings.
    for block in data[128..].as_chunks::<128>().0 {
        if block[0] != 0x02 {
            continue;
        }
        let start = block[2] as usize;
        if start < 4 {
            continue;
        }
        let mut offset = start;
        while offset + 18 <= 127 {
            descriptor(&block[offset..offset + 18], &mut edid);
            offset += 18;
        }
    }
    Some(edid)
}

fn descriptor(d: &[u8], edid: &mut Edid) {
    let clock = u16::from_le_bytes([d[0], d[1]]) as f64 * 10_000.0;
    if clock == 0.0 {
        if d[3] == 0xfc {
            edid.name = d[5..18]
                .iter()
                .take_while(|&&b| b != b'\n')
                .map(|&b| b as char)
                .collect::<String>()
                .trim()
                .to_string();
        }
        return;
    }
    let h_active = d[2] as u32 | ((d[4] as u32 >> 4) << 8);
    let h_blank = d[3] as u32 | ((d[4] as u32 & 0x0f) << 8);
    let v_active = d[5] as u32 | ((d[7] as u32 >> 4) << 8);
    let v_blank = d[6] as u32 | ((d[7] as u32 & 0x0f) << 8);
    let total = (h_active + h_blank) as f64 * (v_active + v_blank) as f64;
    if total == 0.0 || h_active == 0 || v_active == 0 {
        return;
    }
    let mut v = v_active;
    if d[17] & 0x80 != 0 {
        v *= 2; // Zeilensprung: Halbbilder
    }
    edid.timings.push(Mode {
        width: h_active,
        height: v,
        refresh: Some(((clock / total) * 1000.0).round() as f32 / 1000.0),
    });
}

fn standard_timing(b0: u8, b1: u8, version_minor: u8) -> Option<Mode> {
    if (b0 == 0x01 && b1 == 0x01) || b0 == 0 {
        return None;
    }
    let width = (b0 as u32 + 31) * 8;
    let height = match b1 >> 6 {
        0 if version_minor < 3 => width,
        0 => width * 10 / 16,
        1 => width * 3 / 4,
        2 => width * 4 / 5,
        _ => width * 9 / 16,
    };
    Some(Mode {
        width,
        height,
        refresh: Some(((b1 & 0x3f) as u32 + 60) as f32),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// EDID 1.4 mit 2560×1440 @ 143,97 Hz als bevorzugtem Modus, 1920×1080 @ 60 als
    /// Standard-Timing und dem Monitornamen „TUILNUX 27“.
    fn sample() -> Vec<u8> {
        let mut e = vec![0u8; 128];
        e[..8].copy_from_slice(&EDID_HEADER);
        e[18] = 1;
        e[19] = 4;
        e[38..54].copy_from_slice(&[0x01; 16]);
        // 1920×1080 16:9 @ 60 Hz
        e[38] = (1920 / 8 - 31) as u8;
        e[39] = 0b1100_0000;
        // 2560×1440, Pixeltakt 592,25 MHz, htotal 2720, vtotal 1513 → 143,91 Hz
        let clock: u16 = 59225;
        let d = &mut e[54..72];
        d[0..2].copy_from_slice(&clock.to_le_bytes());
        d[2] = (2560 & 0xff) as u8;
        d[3] = (160 & 0xff) as u8;
        d[4] = ((2560 >> 8) << 4) as u8; // 160 >> 8 = 0
        d[5] = (1440 & 0xff) as u8;
        d[6] = 73;
        d[7] = ((1440 >> 8) << 4) as u8;
        let n = &mut e[72..90];
        n[3] = 0xfc;
        n[5..18].copy_from_slice(b"TUILNUX 27\n  ");
        e
    }

    #[test]
    fn parses_detailed_and_standard_timings() {
        let edid = parse_edid(&sample()).unwrap();
        assert_eq!(edid.name, "TUILNUX 27");
        assert_eq!(edid.timings.len(), 2);
        let best = &edid.timings[0];
        assert_eq!((best.width, best.height), (2560, 1440));
        assert!((best.refresh.unwrap() - 143.912).abs() < 0.01);
        assert_eq!(edid.timings[1].key(), "1920x1080@60");
        assert!(parse_edid(&[0; 64]).is_none());
        assert!(parse_edid(&[1; 128]).is_none());
    }

    #[test]
    fn builds_mode_list_with_preferred_first() {
        let out = build(
            "DP-1",
            "2560x1440\n1920x1080\n1920x1080i\n1280x720\n",
            &sample(),
        );
        assert_eq!(out.model, "TUILNUX 27");
        let keys: Vec<String> = out.modes.iter().map(Mode::key).collect();
        assert_eq!(keys, ["2560x1440@144", "1920x1080@60", "1280x720"]);
        assert_eq!(out.best().unwrap().key(), "2560x1440@144");
        assert_eq!(out.modes[0].label(), "2560 × 1440  @ 144 Hz");
    }

    #[test]
    fn works_without_edid() {
        let out = build("Virtual-1", "1280x800\n1920x1080\n800x600\n", &[]);
        let keys: Vec<String> = out.modes.iter().map(Mode::key).collect();
        assert_eq!(keys, ["1280x800", "1920x1080", "800x600"]);
        assert_eq!(out.best().unwrap().key(), "1280x800");
    }

    #[test]
    fn detects_connected_outputs_only() {
        let dir = std::env::temp_dir().join(format!("tuilnux-drm-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for (name, status) in [
            ("card0-HDMI-A-1", "connected"),
            ("card0-DP-2", "disconnected"),
        ] {
            let d = dir.join(name);
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(d.join("status"), format!("{status}\n")).unwrap();
            std::fs::write(d.join("modes"), "1920x1080\n").unwrap();
        }
        std::fs::create_dir_all(dir.join("renderD128")).unwrap();
        let found = detect_in(&dir);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name, "HDMI-A-1");
    }
}
