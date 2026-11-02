//! `.ASCIIA` – das Animationsformat von TUILNUX.
//!
//! ```text
//! # Kommentar
//! [meta]
//! version = 1
//! fps = 12
//! loop = false
//!
//! [palette]
//! accent = #5fd7af
//! blue = #87afff
//!
//! [frame]                       ; Standard: Dauer 1/fps, keine Farbe
//! [frame duration=400 color=accent]
//! [frame gradient=accent,blue]  ; Farbverlauf von oben nach unten
//! ```
//!
//! Alle Zeilen nach einer `[frame …]`-Kopfzeile bis zur nächsten `[`-Kopfzeile
//! sind Inhalt. Eine Inhaltszeile, die mit `[` beginnen soll, wird als `\[` geschrieben.

use std::collections::HashMap;
use std::time::Duration;

pub mod player;

pub const MAX_FILE_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_FRAMES: usize = 10_000;
pub const MAX_WIDTH: usize = 400;
pub const MAX_HEIGHT: usize = 200;
pub const MAX_FPS: u32 = 120;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgb(pub u8, pub u8, pub u8);

impl Rgb {
    pub fn parse(s: &str) -> Option<Rgb> {
        let hex = s.trim().trim_matches('"').strip_prefix('#')?;
        if hex.len() != 6 || !hex.is_ascii() {
            return None;
        }
        let v = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok();
        Some(Rgb(v(0)?, v(2)?, v(4)?))
    }

    pub fn lerp(self, other: Rgb, t: f32) -> Rgb {
        let m = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * t).round() as u8;
        Rgb(m(self.0, other.0), m(self.1, other.1), m(self.2, other.2))
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum FrameColor {
    None,
    Solid(Rgb),
    Gradient(Rgb, Rgb),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Frame {
    pub lines: Vec<String>,
    pub duration: Duration,
    pub color: FrameColor,
}

impl Frame {
    /// Farbe einer Zeile (für Verläufe interpoliert).
    pub fn line_color(&self, row: usize) -> Option<Rgb> {
        match self.color {
            FrameColor::None => None,
            FrameColor::Solid(c) => Some(c),
            FrameColor::Gradient(a, b) => {
                let n = self.lines.len().max(2) - 1;
                Some(a.lerp(b, row.min(n) as f32 / n as f32))
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Animation {
    pub fps: u32,
    pub looping: bool,
    pub width: usize,
    pub height: usize,
    pub frames: Vec<Frame>,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ParseError {
    #[error("Datei ist größer als {MAX_FILE_BYTES} Bytes")]
    TooLarge,
    #[error("Zeile {line}: {msg}")]
    Line { line: usize, msg: String },
    #[error("keine Frames gefunden")]
    NoFrames,
}

fn err(line: usize, msg: impl Into<String>) -> ParseError {
    ParseError::Line {
        line,
        msg: msg.into(),
    }
}

#[derive(PartialEq)]
enum Section {
    None,
    Meta,
    Palette,
    Frame,
}

struct PendingFrame {
    lines: Vec<String>,
    duration: Option<Duration>,
    color: FrameColor,
}

fn parse_key_value(text: &str, line: usize) -> Result<(String, String), ParseError> {
    let (k, v) = text
        .split_once('=')
        .ok_or_else(|| err(line, "erwartet `schlüssel = wert`"))?;
    Ok((
        k.trim().to_lowercase(),
        v.trim().trim_matches('"').to_string(),
    ))
}

fn parse_duration(v: &str, line: usize) -> Result<Duration, ParseError> {
    let v = v.trim();
    let (num, mult) = if let Some(n) = v.strip_suffix("ms") {
        (n, 1)
    } else if let Some(n) = v.strip_suffix('s') {
        (n, 1000)
    } else {
        (v, 1)
    };
    let ms: u64 = num
        .trim()
        .parse()
        .map_err(|_| err(line, format!("ungültige Dauer `{v}`")))?;
    let ms = ms * mult;
    if !(1..=60_000).contains(&ms) {
        return Err(err(line, "Dauer muss zwischen 1 ms und 60 s liegen"));
    }
    Ok(Duration::from_millis(ms))
}

fn resolve_color(
    name: &str,
    palette: &HashMap<String, Rgb>,
    line: usize,
) -> Result<Rgb, ParseError> {
    Rgb::parse(name)
        .or_else(|| palette.get(&name.to_lowercase()).copied())
        .ok_or_else(|| err(line, format!("unbekannte Farbe `{name}`")))
}

pub fn parse(source: &str) -> Result<Animation, ParseError> {
    if source.len() > MAX_FILE_BYTES {
        return Err(ParseError::TooLarge);
    }
    let mut fps = 12u32;
    let mut looping = false;
    let mut palette = HashMap::new();
    let mut frames: Vec<PendingFrame> = Vec::new();
    let mut section = Section::None;

    for (idx, raw) in source.lines().enumerate() {
        let n = idx + 1;
        let trimmed = raw.trim();

        if trimmed.starts_with('[') && !raw.starts_with("\\[") {
            let header = trimmed
                .strip_prefix('[')
                .and_then(|h| h.strip_suffix(']'))
                .ok_or_else(|| err(n, "Kopfzeile muss mit `]` enden"))?;
            let mut parts = header.split_whitespace();
            match parts.next().unwrap_or("").to_lowercase().as_str() {
                "meta" => section = Section::Meta,
                "palette" => section = Section::Palette,
                "frame" => {
                    if frames.len() >= MAX_FRAMES {
                        return Err(err(n, format!("mehr als {MAX_FRAMES} Frames")));
                    }
                    let mut frame = PendingFrame {
                        lines: Vec::new(),
                        duration: None,
                        color: FrameColor::None,
                    };
                    for attr in parts {
                        let (k, v) = parse_key_value(attr, n)?;
                        match k.as_str() {
                            "duration" => frame.duration = Some(parse_duration(&v, n)?),
                            "color" => {
                                frame.color = FrameColor::Solid(resolve_color(&v, &palette, n)?)
                            }
                            "gradient" => {
                                let (a, b) = v
                                    .split_once(',')
                                    .ok_or_else(|| err(n, "gradient=farbe1,farbe2"))?;
                                frame.color = FrameColor::Gradient(
                                    resolve_color(a, &palette, n)?,
                                    resolve_color(b, &palette, n)?,
                                );
                            }
                            other => return Err(err(n, format!("unbekanntes Attribut `{other}`"))),
                        }
                    }
                    frames.push(frame);
                    section = Section::Frame;
                }
                other => return Err(err(n, format!("unbekannter Abschnitt `{other}`"))),
            }
            continue;
        }

        match section {
            Section::Frame => {
                let content = raw
                    .strip_prefix('\\')
                    .filter(|r| r.starts_with('['))
                    .unwrap_or(raw);
                let content = content.trim_end_matches('\r').replace('\t', "    ");
                if content.chars().count() > MAX_WIDTH {
                    return Err(err(n, format!("Zeile breiter als {MAX_WIDTH} Zeichen")));
                }
                let frame = frames.last_mut().expect("frame section has a frame");
                if frame.lines.len() >= MAX_HEIGHT {
                    return Err(err(n, format!("Frame höher als {MAX_HEIGHT} Zeilen")));
                }
                frame.lines.push(content);
            }
            _ if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with(';') => {}
            Section::Meta => {
                let (k, v) = parse_key_value(trimmed, n)?;
                match k.as_str() {
                    "version" if v == "1" => {}
                    "version" => {
                        return Err(err(n, format!("Version `{v}` wird nicht unterstützt")));
                    }
                    "fps" => {
                        fps = v
                            .parse()
                            .ok()
                            .filter(|f| (1..=MAX_FPS).contains(f))
                            .ok_or_else(|| err(n, format!("fps muss 1–{MAX_FPS} sein")))?;
                    }
                    "loop" => {
                        looping = match v.as_str() {
                            "true" | "yes" | "1" => true,
                            "false" | "no" | "0" => false,
                            _ => return Err(err(n, "loop muss true oder false sein")),
                        }
                    }
                    "name" | "author" | "size" | "description" => {}
                    other => return Err(err(n, format!("unbekannter Schlüssel `{other}`"))),
                }
            }
            Section::Palette => {
                let (k, v) = parse_key_value(trimmed, n)?;
                let rgb = Rgb::parse(&v).ok_or_else(|| err(n, format!("ungültige Farbe `{v}`")))?;
                palette.insert(k, rgb);
            }
            Section::None => return Err(err(n, "Inhalt außerhalb eines Abschnitts")),
        }
    }

    if frames.is_empty() {
        return Err(ParseError::NoFrames);
    }

    let default = Duration::from_millis(1000 / fps as u64);
    let frames: Vec<Frame> = frames
        .into_iter()
        .map(|mut f| {
            while f.lines.last().is_some_and(|l| l.trim().is_empty()) {
                f.lines.pop();
            }
            Frame {
                lines: f.lines,
                duration: f.duration.unwrap_or(default),
                color: f.color,
            }
        })
        .collect();
    let width = frames
        .iter()
        .flat_map(|f| f.lines.iter())
        .map(|l| l.chars().count())
        .max()
        .unwrap_or(0);
    let height = frames.iter().map(|f| f.lines.len()).max().unwrap_or(0);

    Ok(Animation {
        fps,
        looping,
        width,
        height,
        frames,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
# Test
[meta]
version = 1
fps = 10
loop = true

[palette]
a = #5fd7af
b = #000000

[frame color=a]
 /\\
/  \\

[frame duration=250ms gradient=a,b]
\\[x]
line2
";

    #[test]
    fn parses_sample() {
        let anim = parse(SAMPLE).unwrap();
        assert_eq!(anim.fps, 10);
        assert!(anim.looping);
        assert_eq!(anim.frames.len(), 2);
        assert_eq!(anim.frames[0].lines, vec![" /\\", "/  \\"]);
        assert_eq!(anim.frames[0].duration, Duration::from_millis(100));
        assert_eq!(
            anim.frames[0].color,
            FrameColor::Solid(Rgb(0x5f, 0xd7, 0xaf))
        );
        assert_eq!(anim.frames[1].lines[0], "[x]");
        assert_eq!(anim.frames[1].duration, Duration::from_millis(250));
        assert_eq!(anim.frames[1].line_color(1), Some(Rgb(0, 0, 0)));
        assert_eq!((anim.width, anim.height), (5, 2));
    }

    #[test]
    fn reports_line_numbers() {
        let e = parse("[meta]\nfps = 999\n[frame]\nx").unwrap_err();
        assert_eq!(e, err(2, format!("fps muss 1–{MAX_FPS} sein")));
        assert!(matches!(
            parse("[frame color=nope]\nx"),
            Err(ParseError::Line { line: 1, .. })
        ));
        assert_eq!(parse("[meta]\nfps = 5"), Err(ParseError::NoFrames));
        assert!(matches!(
            parse("hallo"),
            Err(ParseError::Line { line: 1, .. })
        ));
    }

    #[test]
    fn rejects_oversized_lines() {
        let src = format!("[frame]\n{}", "x".repeat(MAX_WIDTH + 1));
        assert!(matches!(parse(&src), Err(ParseError::Line { line: 2, .. })));
    }

    #[test]
    fn bundled_assets_are_valid() {
        for src in [
            include_str!("../../../assets/welcome.ASCIIA"),
            include_str!("../../../assets/loading.ASCIIA"),
        ] {
            parse(src).unwrap();
        }
    }
}
