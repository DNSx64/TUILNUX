use std::process::ExitCode;

use anyhow::{Result, bail};
use tuilnux_media::{FitMode, WallpaperConfig, expand_home, load_image};

const HELP: &str = "\
wallpaper – Hintergrundbild hinter dem TUILNUX-Terminal setzen

Verwendung:
  wallpaper set <bild> [--dim 0.0-1.0] [--fill | --fit | --center]
  wallpaper off
  wallpaper show

Die Einstellung wird in ~/.config/tuilnux/wallpaper.toml gespeichert und vom
Compositor übernommen. In ~/.bashrc z. B.:
  wallpaper set ~/Bilder/berge.png --dim 0.6";

fn run(args: &[String]) -> Result<()> {
    let mut cfg = WallpaperConfig::load()?;
    match args.first().map(String::as_str) {
        Some("set") => {
            let mut path = None;
            let mut it = args[1..].iter();
            while let Some(a) = it.next() {
                match a.as_str() {
                    "--dim" => {
                        let Some(d) = it
                            .next()
                            .and_then(|v| v.parse::<f32>().ok())
                            .filter(|d| (0.0..=1.0).contains(d))
                        else {
                            bail!("--dim erwartet eine Zahl zwischen 0.0 und 1.0");
                        };
                        cfg.dim = d;
                    }
                    "--fill" => cfg.mode = FitMode::Fill,
                    "--fit" => cfg.mode = FitMode::Fit,
                    "--center" => cfg.mode = FitMode::Center,
                    s if s.starts_with('-') => bail!("unbekannte Option `{s}`"),
                    s => path = Some(expand_home(s)),
                }
            }
            let Some(path) = path else {
                bail!("kein Bild angegeben (wallpaper set <bild>)")
            };
            let path = std::fs::canonicalize(&path).unwrap_or(path);
            let img = load_image(&path)?;
            cfg.path = Some(path.clone());
            let file = cfg.save()?;
            println!(
                "Hintergrundbild: {} ({}x{}, Abdunklung {:.0} %) → {}",
                path.display(),
                img.width(),
                img.height(),
                cfg.dim * 100.0,
                file.display()
            );
        }
        Some("off") => {
            cfg.path = None;
            cfg.save()?;
            println!("Hintergrundbild entfernt.");
        }
        Some("show") | None => match &cfg.path {
            Some(p) => println!(
                "{} (Abdunklung {:.0} %, {:?})",
                p.display(),
                cfg.dim * 100.0,
                cfg.mode
            ),
            None => println!("Kein Hintergrundbild gesetzt."),
        },
        Some("-h" | "--help") => println!("{HELP}"),
        Some(other) => bail!("unbekannter Befehl `{other}` (siehe --help)"),
    }
    Ok(())
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("wallpaper: {e:#}");
            ExitCode::FAILURE
        }
    }
}
