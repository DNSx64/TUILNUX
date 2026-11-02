use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};

use anyhow::{Context, Result, bail};
use image::DynamicImage;
use tuilnux_media::{
    Protocol, decode_image, expand_home, fit_cells, halfblock_lines, kitty_sequence, load_image,
    paths_from_text,
};

const HELP: &str = "\
view – Bilder direkt im Terminal anzeigen

Verwendung:
  view <bild>... [--width <spalten>] [--height <zeilen>] [--kitty | --half]
  view --paste [--no-save]     Bild aus der Zwischenablage einfügen (auch: bild, Alt+V)

  --paste   kopiertes Bild (z. B. aus Firefox) oder kopierte Bilddatei anzeigen;
            kopierte Bilddaten werden unter ~/Bilder/Eingefügt-DATUM.<format> gespeichert
  --no-save eingefügte Bilddaten nur anzeigen, nicht speichern
  --kitty   Kitty-Grafikprotokoll erzwingen (pixelgenau)
  --half    Halbblock-Darstellung erzwingen (funktioniert in jedem Truecolor-Terminal)
Das Format wird am Inhalt erkannt – die Dateiendung ist egal (auch file://-Links).
Ohne Angabe wird das Protokoll automatisch erkannt.";

fn main() -> ExitCode {
    let mut files = Vec::new();
    let mut width: Option<u16> = None;
    let mut height: Option<u16> = None;
    let mut protocol = None;
    let mut paste = false;
    let mut save = true;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => {
                println!("{HELP}");
                return ExitCode::SUCCESS;
            }
            "--kitty" => protocol = Some(Protocol::Kitty),
            "--half" => protocol = Some(Protocol::HalfBlock),
            "-p" | "--paste" => paste = true,
            "--no-save" => save = false,
            "-w" | "--width" | "--height" => {
                let Some(v) = args.next().and_then(|v| v.parse().ok()).filter(|v| *v > 0) else {
                    eprintln!("view: {arg} erwartet eine positive Zahl");
                    return ExitCode::from(2);
                };
                if arg == "--height" {
                    height = Some(v)
                } else {
                    width = Some(v)
                }
            }
            s if s.starts_with('-') => {
                eprintln!("view: unbekannte Option `{s}` (siehe --help)");
                return ExitCode::from(2);
            }
            s if s.starts_with("file://") => files.extend(paths_from_text(s)),
            s => files.push(expand_home(s)),
        }
    }

    let mut images: Vec<(String, Result<DynamicImage>)> = files
        .iter()
        .map(|f| (f.display().to_string(), load_image(f)))
        .collect();
    if paste {
        match from_clipboard(save) {
            Ok(found) => images.extend(found),
            Err(e) => {
                eprintln!("view: {e:#}");
                return ExitCode::FAILURE;
            }
        }
    }
    if images.is_empty() {
        eprintln!("{HELP}");
        return ExitCode::from(2);
    }

    let (term_cols, term_rows) = crossterm::terminal::size().unwrap_or((80, 24));
    let max_cols = width.unwrap_or(term_cols).min(term_cols);
    let max_rows = height.unwrap_or(term_rows.saturating_sub(2).max(1));
    let protocol = protocol.unwrap_or_else(Protocol::detect);
    let mut out = std::io::stdout().lock();
    let mut failed = false;
    let many = images.len() > 1;

    for (label, img) in images {
        let img = match img {
            Ok(img) => img,
            Err(e) => {
                eprintln!("view: {e:#}");
                failed = true;
                continue;
            }
        };
        let (cols, rows) = fit_cells(img.width(), img.height(), max_cols, max_rows);
        let result = match protocol {
            Protocol::Kitty if out.is_terminal() => {
                kitty_sequence(&img, cols, rows).and_then(|seq| Ok(writeln!(out, "{seq}")?))
            }
            _ => halfblock_lines(&img, cols, rows)
                .iter()
                .try_for_each(|line| writeln!(out, "{line}"))
                .map_err(Into::into),
        };
        if let Err(e) = result {
            eprintln!("view: {e:#}");
            failed = true;
        }
        if many || paste {
            let _ = writeln!(
                out,
                "\x1b[2m{label}  ({}x{})\x1b[0m",
                img.width(),
                img.height()
            );
        }
    }
    let _ = out.flush();
    if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

fn wl_paste(args: &[&str]) -> Result<Vec<u8>> {
    let out = Command::new("wl-paste")
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .context("wl-paste fehlt – Paket wl-clipboard installieren")?;
    if !out.status.success() {
        bail!("Zwischenablage ist leer");
    }
    Ok(out.stdout)
}

/// Bild aus der Wayland-Zwischenablage: zuerst Bilddaten (image/*), sonst
/// kopierte Dateien (text/uri-list, z. B. aus einem Dateimanager) oder ein Pfad.
fn from_clipboard(save: bool) -> Result<Vec<(String, Result<DynamicImage>)>> {
    if std::env::var_os("WAYLAND_DISPLAY").is_none() {
        bail!(
            "keine Zwischenablage: Einfügen braucht die grafische Sitzung (labwc). Auf der Konsole: view <datei>"
        );
    }
    let types = String::from_utf8_lossy(&wl_paste(&["--list-types"])?).into_owned();
    let types: Vec<&str> = types.lines().map(str::trim).collect();
    if let Some(mime) = types.iter().find(|t| t.starts_with("image/")) {
        let bytes = wl_paste(&["--no-newline", "--type", mime])?;
        let (img, format) = decode_image(&bytes)?;
        let label = if save {
            let path = save_pasted(&bytes, format)?;
            format!("gespeichert: {}", path.display())
        } else {
            format!("Zwischenablage ({mime})")
        };
        return Ok(vec![(label, Ok(img))]);
    }
    let text = if types.contains(&"text/uri-list") {
        wl_paste(&["--no-newline", "--type", "text/uri-list"])?
    } else if types
        .iter()
        .any(|t| t.starts_with("text/") || *t == "UTF8_STRING")
    {
        wl_paste(&["--no-newline"])?
    } else {
        bail!("die Zwischenablage enthält kein Bild");
    };
    let files: Vec<PathBuf> = paths_from_text(&String::from_utf8_lossy(&text))
        .into_iter()
        .filter(|p| p.is_file())
        .collect();
    if files.is_empty() {
        bail!("die Zwischenablage enthält kein Bild und keine Bilddatei");
    }
    Ok(files
        .iter()
        .map(|f| (f.display().to_string(), load_image(f)))
        .collect())
}

fn pictures_dir() -> PathBuf {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    dirs::picture_dir()
        .or_else(|| {
            ["Bilder", "Pictures"]
                .iter()
                .map(|d| home.join(d))
                .find(|d| d.is_dir())
        })
        .unwrap_or_else(|| home.join("Bilder"))
}

fn stamp() -> String {
    Command::new("date")
        .arg("+%Y-%m-%d_%H-%M-%S")
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_secs())
                .to_string()
        })
}

fn save_pasted(bytes: &[u8], format: image::ImageFormat) -> Result<PathBuf> {
    let dir = pictures_dir();
    std::fs::create_dir_all(&dir).with_context(|| dir.display().to_string())?;
    let ext = format.extensions_str().first().copied().unwrap_or("bild");
    let base = format!("Eingefügt-{}", stamp());
    let mut path = dir.join(format!("{base}.{ext}"));
    let mut n = 1;
    while path.exists() {
        n += 1;
        path = dir.join(format!("{base}-{n}.{ext}"));
    }
    write_new(&path, bytes)?;
    Ok(path)
}

fn write_new(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .with_context(|| path.display().to_string())?;
    f.write_all(bytes)?;
    Ok(())
}
