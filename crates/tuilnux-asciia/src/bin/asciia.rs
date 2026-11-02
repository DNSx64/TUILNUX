use std::io::IsTerminal;
use std::path::PathBuf;
use std::process::ExitCode;

use tuilnux_asciia::player::{self, Mode, Options};

const HELP: &str = "\
asciia – .ASCIIA-Animationen im Terminal abspielen

Verwendung:
  asciia <datei> [--once | --loop] [--speed <faktor>] [--clear]
  asciia --check <datei>

Optionen:
  --once         einmal abspielen, letzter Frame bleibt stehen
  --loop         endlos abspielen, beenden mit Ctrl+C / q
                 (ohne Angabe gilt `loop` aus der Datei)
  --speed <f>    Wiedergabegeschwindigkeit, z. B. 0.5 oder 2
  --clear        Animation nach dem Ende entfernen
  --force        auch abspielen, wenn stdout kein Terminal ist
  --check        Datei nur prüfen und Infos ausgeben

In ~/.bashrc:
  asciia ~/.config/tuilnux/welcome.ASCIIA --once";

fn expand_home(path: &str) -> PathBuf {
    match path.strip_prefix("~/") {
        Some(rest) => std::env::home_dir().map_or_else(|| PathBuf::from(path), |h| h.join(rest)),
        None => PathBuf::from(path),
    }
}

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let mut file = None;
    let mut mode = None;
    let mut speed = 1.0f32;
    let mut keep_last = true;
    let mut force = false;
    let mut check = false;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => {
                println!("{HELP}");
                return ExitCode::SUCCESS;
            }
            "--once" => mode = Some(Mode::Once),
            "--loop" => mode = Some(Mode::Loop),
            "--clear" => keep_last = false,
            "--force" => force = true,
            "--check" => check = true,
            "--speed" => match args.next().and_then(|s| s.parse::<f32>().ok()) {
                Some(s) if s > 0.0 => speed = s,
                _ => {
                    eprintln!("asciia: --speed erwartet eine positive Zahl");
                    return ExitCode::from(2);
                }
            },
            s if s.starts_with('-') => {
                eprintln!("asciia: unbekannte Option `{s}` (siehe --help)");
                return ExitCode::from(2);
            }
            s => file = Some(expand_home(s)),
        }
    }

    let Some(file) = file else {
        eprintln!("{HELP}");
        return ExitCode::from(2);
    };
    let source = match std::fs::read_to_string(&file) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("asciia: {}: {e}", file.display());
            return ExitCode::FAILURE;
        }
    };
    let anim = match tuilnux_asciia::parse(&source) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("asciia: {}: {e}", file.display());
            return ExitCode::FAILURE;
        }
    };

    if check {
        println!(
            "{}: ok – {} Frames, {}x{} Zeichen, {} fps, loop={}",
            file.display(),
            anim.frames.len(),
            anim.width,
            anim.height,
            anim.fps,
            anim.looping
        );
        return ExitCode::SUCCESS;
    }

    // Nicht-interaktive Shells (scp, rsync, Pipes) nicht stören.
    if !force && !std::io::stdout().is_terminal() {
        return ExitCode::SUCCESS;
    }

    let mode = mode.unwrap_or(if anim.looping { Mode::Loop } else { Mode::Once });
    match player::play(
        &anim,
        Options {
            mode,
            speed,
            keep_last,
        },
    ) {
        Ok(true) => ExitCode::from(130),
        Ok(false) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("asciia: {e}");
            ExitCode::FAILURE
        }
    }
}
