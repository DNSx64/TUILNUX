//! Bilder im Terminal: Halbblock-Rendering (läuft überall mit 24-Bit-Farbe)
//! und das Kitty-Grafikprotokoll für pixelgenaue Darstellung.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use base64::Engine;
use image::imageops::FilterType;
use image::{DynamicImage, GenericImageView, Rgba};
use serde::{Deserialize, Serialize};

pub const MAX_IMAGE_BYTES: u64 = 64 * 1024 * 1024;
pub const MAX_IMAGE_PIXELS: u64 = 64_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Protocol {
    Kitty,
    HalfBlock,
}

impl Protocol {
    /// Erkennt das beste verfügbare Grafikprotokoll anhand der Umgebung.
    pub fn detect() -> Protocol {
        let var = |k: &str| std::env::var(k).unwrap_or_default();
        let term = var("TERM");
        let program = var("TERM_PROGRAM");
        if term.contains("kitty")
            || std::env::var_os("KITTY_WINDOW_ID").is_some()
            || program == "WezTerm"
            || program == "ghostty"
            || program == "tuilnux-term"
        {
            Protocol::Kitty
        } else {
            Protocol::HalfBlock
        }
    }
}

pub fn expand_home(path: &str) -> PathBuf {
    match path.strip_prefix("~/") {
        Some(rest) => dirs::home_dir().map_or_else(|| PathBuf::from(path), |h| h.join(rest)),
        None => PathBuf::from(path),
    }
}

/// Lädt ein Bild mit Größenlimits.
pub fn load_image(path: &Path) -> Result<DynamicImage> {
    let meta = std::fs::metadata(path).with_context(|| path.display().to_string())?;
    if meta.len() > MAX_IMAGE_BYTES {
        bail!(
            "{}: Datei größer als {} MiB",
            path.display(),
            MAX_IMAGE_BYTES / 1024 / 1024
        );
    }
    let reader = image::ImageReader::open(path)
        .with_context(|| path.display().to_string())?
        .with_guessed_format()?;
    let (w, h) = reader
        .into_dimensions()
        .with_context(|| format!("{}: kein unterstütztes Bildformat", path.display()))?;
    if w as u64 * h as u64 > MAX_IMAGE_PIXELS {
        bail!("{}: Bild zu groß ({w}x{h})", path.display());
    }
    image::ImageReader::open(path)?
        .with_guessed_format()?
        .decode()
        .with_context(|| format!("{}: Bild konnte nicht dekodiert werden", path.display()))
}

/// Dekodiert ein Bild aus dem Speicher (z. B. aus der Zwischenablage); das
/// Format wird am Inhalt erkannt, nicht an einer Endung.
pub fn decode_image(bytes: &[u8]) -> Result<(DynamicImage, image::ImageFormat)> {
    if bytes.len() as u64 > MAX_IMAGE_BYTES {
        bail!("Bild größer als {} MiB", MAX_IMAGE_BYTES / 1024 / 1024);
    }
    let format = image::guess_format(bytes).context("kein unterstütztes Bildformat")?;
    let reader = image::ImageReader::with_format(std::io::Cursor::new(bytes), format);
    let (w, h) = reader.into_dimensions()?;
    if w as u64 * h as u64 > MAX_IMAGE_PIXELS {
        bail!("Bild zu groß ({w}x{h})");
    }
    let img = image::load_from_memory_with_format(bytes, format)
        .context("Bild konnte nicht dekodiert werden")?;
    Ok((img, format))
}

/// Dateien aus kopiertem Text: `text/uri-list` (`file:///…`, Prozent-kodiert),
/// einfache Pfade, auch in Anführungszeichen oder mit `~`.
pub fn paths_from_text(text: &str) -> Vec<PathBuf> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(|l| {
            let l = l.trim_matches(|c| c == '\'' || c == '"');
            if let Some(rest) = l.strip_prefix("file://") {
                // file://host/pfad → nur lokale Dateien
                let path = rest.strip_prefix("localhost").unwrap_or(rest);
                path.starts_with('/')
                    .then(|| PathBuf::from(percent_decode(path)))
            } else if l.contains("://") {
                None
            } else {
                Some(expand_home(&l.replace("\\ ", " ")))
            }
        })
        .collect()
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && bytes[i + 1].is_ascii_hexdigit()
            && bytes[i + 2].is_ascii_hexdigit()
            && let Ok(b) = u8::from_str_radix(&s[i + 1..i + 3], 16)
        {
            out.push(b);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Zielgröße in Terminalzellen bei gleichem Seitenverhältnis
/// (eine Zelle ≈ doppelt so hoch wie breit).
pub fn fit_cells(img_w: u32, img_h: u32, max_cols: u16, max_rows: u16) -> (u16, u16) {
    let max_cols = max_cols.max(1) as f64;
    let max_rows = max_rows.max(1) as f64;
    let aspect = img_h as f64 / img_w.max(1) as f64;
    let mut cols = max_cols.min(img_w as f64);
    let mut rows = cols * aspect / 2.0;
    if rows > max_rows {
        rows = max_rows;
        cols = rows * 2.0 / aspect;
    }
    (cols.round().max(1.0) as u16, rows.round().max(1.0) as u16)
}

fn blend(px: Rgba<u8>) -> Rgb {
    // Transparente Pixel auf Schwarz (Terminal-Hintergrund) abbilden.
    let a = px[3] as u16;
    let m = |c: u8| ((c as u16 * a) / 255) as u8;
    (m(px[0]), m(px[1]), m(px[2]))
}

pub type Rgb = (u8, u8, u8);

/// Halbblock-Zellen als (obere, untere) Farbe – für TUI-Apps, die selbst zeichnen.
pub fn halfblock_cells(img: &DynamicImage, cols: u16, rows: u16) -> Vec<Vec<(Rgb, Rgb)>> {
    let small = img.resize_exact(cols as u32, rows as u32 * 2, FilterType::Triangle);
    (0..rows as u32)
        .map(|y| {
            (0..cols as u32)
                .map(|x| {
                    (
                        blend(small.get_pixel(x, y * 2)),
                        blend(small.get_pixel(x, y * 2 + 1)),
                    )
                })
                .collect()
        })
        .collect()
}

/// Rendert das Bild als Zeilen aus `▀`-Zeichen: Vordergrund = obere, Hintergrund = untere Pixelhälfte.
pub fn halfblock_lines(img: &DynamicImage, cols: u16, rows: u16) -> Vec<String> {
    halfblock_cells(img, cols, rows)
        .into_iter()
        .map(|row| {
            let mut line = String::with_capacity(cols as usize * 40);
            for ((tr, tg, tb), (br, bg, bb)) in row {
                let _ = write!(line, "\x1b[38;2;{tr};{tg};{tb}m\x1b[48;2;{br};{bg};{bb}m▀");
            }
            line.push_str("\x1b[0m");
            line
        })
        .collect()
}

/// Erzeugt die Kitty-Grafik-Escape-Sequenz (PNG, in 4096-Byte-Blöcken übertragen).
pub fn kitty_sequence(img: &DynamicImage, cols: u16, rows: u16) -> Result<String> {
    let max_w = (cols as u32 * 16).min(img.width());
    let scaled = if img.width() > max_w {
        img.resize(max_w, u32::MAX, FilterType::Lanczos3)
    } else {
        img.clone()
    };
    let mut png = Vec::new();
    scaled.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)?;
    let data = base64::engine::general_purpose::STANDARD.encode(png);
    let chunks: Vec<&str> = data
        .as_bytes()
        .chunks(4096)
        .map(|c| std::str::from_utf8(c).expect("base64 is ascii"))
        .collect();
    let mut out = String::with_capacity(data.len() + chunks.len() * 32);
    for (i, chunk) in chunks.iter().enumerate() {
        let more = u8::from(i + 1 < chunks.len());
        if i == 0 {
            let _ = write!(
                out,
                "\x1b_Ga=T,f=100,q=2,c={cols},r={rows},m={more};{chunk}\x1b\\"
            );
        } else {
            let _ = write!(out, "\x1b_Gm={more};{chunk}\x1b\\");
        }
    }
    Ok(out)
}

/// Konfiguration des Hintergrundbildes (`~/.config/tuilnux/wallpaper.toml`).
/// Der Compositor liest diese Datei beim Start und bei Änderungen.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WallpaperConfig {
    pub path: Option<PathBuf>,
    /// Abdunklung 0.0 (keine) bis 1.0 (schwarz), damit Text lesbar bleibt.
    pub dim: f32,
    pub mode: FitMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FitMode {
    Fill,
    Fit,
    Center,
}

impl Default for WallpaperConfig {
    fn default() -> Self {
        Self {
            path: None,
            dim: 0.55,
            mode: FitMode::Fill,
        }
    }
}

impl WallpaperConfig {
    pub fn file() -> Result<PathBuf> {
        let dir = dirs::config_dir().context("kein Konfigurationsverzeichnis gefunden")?;
        Ok(dir.join("tuilnux").join("wallpaper.toml"))
    }

    pub fn load() -> Result<Self> {
        let file = Self::file()?;
        match std::fs::read_to_string(&file) {
            Ok(s) => toml::from_str(&s).with_context(|| file.display().to_string()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e).with_context(|| file.display().to_string()),
        }
    }

    pub fn save(&self) -> Result<PathBuf> {
        let file = Self::file()?;
        std::fs::create_dir_all(file.parent().expect("has parent"))?;
        std::fs::write(&file, toml::to_string(self)?)?;
        Ok(file)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_from_copied_text() {
        let p = paths_from_text(
            "# kommentar\nfile:///home/user/Bilder/Mein%20Bild.webp\r\nfile://localhost/tmp/a\n'/tmp/x y'\nhttps://example.com/a.png\n/tmp/b\\ c\n",
        );
        let p: Vec<String> = p.iter().map(|p| p.display().to_string()).collect();
        assert_eq!(
            p,
            [
                "/home/user/Bilder/Mein Bild.webp",
                "/tmp/a",
                "/tmp/x y",
                "/tmp/b c"
            ]
        );
        assert_eq!(percent_decode("%C3%A4%2"), "ä%2");
    }

    #[test]
    fn decodes_images_by_content() {
        let img = DynamicImage::new_rgb8(3, 2);
        let mut png = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let (decoded, format) = decode_image(&png).unwrap();
        assert_eq!(format, image::ImageFormat::Png);
        assert_eq!((decoded.width(), decoded.height()), (3, 2));
        assert!(decode_image(b"kein bild").is_err());
    }
    use image::RgbaImage;

    #[test]
    fn fit_keeps_aspect_ratio() {
        assert_eq!(fit_cells(1600, 900, 80, 100), (80, 23));
        assert_eq!(fit_cells(1000, 1000, 200, 20), (40, 20));
        assert_eq!(fit_cells(10, 10, 80, 40), (10, 5));
    }

    #[test]
    fn halfblock_uses_top_and_bottom_pixel() {
        let mut img = RgbaImage::new(1, 2);
        img.put_pixel(0, 0, Rgba([255, 0, 0, 255]));
        img.put_pixel(0, 1, Rgba([0, 0, 255, 255]));
        let lines = halfblock_lines(&DynamicImage::ImageRgba8(img), 1, 1);
        assert_eq!(lines, vec!["\x1b[38;2;255;0;0m\x1b[48;2;0;0;255m▀\x1b[0m"]);
    }

    #[test]
    fn kitty_sequence_is_chunked() {
        let img = DynamicImage::ImageRgba8(RgbaImage::from_fn(300, 300, |x, y| {
            Rgba([
                (x * 7 % 256) as u8,
                (y * 13 % 256) as u8,
                ((x ^ y) % 256) as u8,
                255,
            ])
        }));
        let seq = kitty_sequence(&img, 20, 10).unwrap();
        assert!(seq.starts_with("\x1b_Ga=T,f=100,q=2,c=20,r=10,m=1;"));
        assert!(seq.ends_with("\x1b\\"));
        assert!(seq.contains("\x1b_Gm=0;"));
    }

    #[test]
    fn wallpaper_config_roundtrip() {
        let cfg = WallpaperConfig {
            path: Some("/tmp/a.png".into()),
            dim: 0.3,
            mode: FitMode::Fit,
        };
        let text = toml::to_string(&cfg).unwrap();
        assert_eq!(toml::from_str::<WallpaperConfig>(&text).unwrap(), cfg);
    }
}
