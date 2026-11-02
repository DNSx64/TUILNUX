//! Führt die Installationsschritte im Hintergrund aus und liefert deren Ausgabe
//! Zeile für Zeile an die Oberfläche.

use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::Duration;

pub enum Msg {
    Line(String),
    Finished(Result<(), String>),
}

/// Die Auswahl des Assistenten, übersetzt in Optionen für `install.sh`.
#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    pub graphical: bool,
    pub greetd: bool,
    pub enable: bool,
    pub bashrc: bool,
    pub font: bool,
    pub nvidia: bool,
    pub tests: bool,
    pub backend: &'static str,
    pub locale: String,
    pub keymap: String,
}

impl Plan {
    pub fn args(&self) -> Vec<String> {
        let mut a: Vec<String> = vec!["--yes".into(), "--text".into()];
        let flags = [
            (!self.graphical, "--minimal"),
            (!self.greetd, "--no-greetd"),
            (!self.enable, "--no-enable"),
            (!self.bashrc, "--no-bashrc"),
            (!self.font, "--no-font"),
            (!self.tests, "--no-tests"),
        ];
        a.extend(flags.iter().filter(|f| f.0).map(|f| f.1.to_string()));
        a.push(
            if self.nvidia {
                "--nvidia"
            } else {
                "--no-nvidia"
            }
            .into(),
        );
        a.extend(["--backend".into(), self.backend.into()]);
        if !self.locale.is_empty() {
            a.extend(["--lang".into(), self.locale.clone()]);
        }
        if !self.keymap.is_empty() {
            a.extend(["--keymap".into(), self.keymap.clone()]);
        }
        a
    }
}

/// Nur Sprache und Tastatur des Systems setzen (TUILNUX ist schon installiert).
const SYSTEM_SCRIPT: &str = r#"
set -e
loc=$1 km=$2
if [ -n "$loc" ]; then
    echo "==> Systemsprache: $loc"
    if [ -f /etc/locale.gen ] && ! grep -q "^$loc " /etc/locale.gen; then
        sed -i "s/^#\s*\($loc \)/\1/" /etc/locale.gen
        locale-gen
    fi
    if command -v localectl >/dev/null; then localectl set-locale "LANG=$loc"; else echo "LANG=$loc" > /etc/locale.conf; fi
fi
if [ -n "$km" ]; then
    echo "==> Tastatur (Konsole): $km"
    if command -v localectl >/dev/null; then localectl set-keymap --no-convert "$km"; fi
fi
"#;

pub fn install_command(root: &Path, plan: &Plan) -> Command {
    let mut cmd = Command::new("bash");
    cmd.arg(root.join("install.sh")).args(plan.args());
    cmd
}

pub fn system_command(plan: &Plan) -> Command {
    let mut cmd = Command::new("sudo");
    cmd.args(["-n", "bash", "-c", SYSTEM_SCRIPT, "bash"])
        .arg(&plan.locale)
        .arg(&plan.keymap);
    cmd
}

/// Zählt die Schritte (`==>`-Zeilen) eines Probelaufs für die Fortschrittsanzeige.
pub fn estimate_steps(root: &Path, plan: &Plan) -> usize {
    let out = Command::new("bash")
        .arg(root.join("install.sh"))
        .args(plan.args())
        .arg("--dry-run")
        .env("TUILNUX_PROGRESS", "1")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output();
    match out {
        Ok(o) => String::from_utf8_lossy(&o.stdout)
            .lines()
            .filter(|l| is_step(l))
            .count()
            .max(1),
        Err(_) => 12,
    }
}

/// Schrittmarke von `install.sh` mit `TUILNUX_PROGRESS=1`.
const STEP_MARK: &str = "##> ";

pub fn is_step(line: &str) -> bool {
    line.starts_with(STEP_MARK)
}

/// Zeile fürs Protokoll: die Schrittmarke wird wieder zum gewohnten `==>`.
pub fn display_line(line: String) -> String {
    match line.strip_prefix(STEP_MARK) {
        Some(rest) => format!("==> {rest}"),
        None => line,
    }
}

pub struct Runner {
    pub rx: Receiver<Msg>,
    stop: Arc<AtomicBool>,
}

impl Drop for Runner {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// Startet die Befehle nacheinander; hält währenddessen die sudo-Anmeldung frisch,
/// damit lange Bauschritte nicht an einer abgelaufenen Passwortfrage hängen.
pub fn start(commands: Vec<Command>) -> Runner {
    let (tx, rx) = mpsc::channel();
    let stop = Arc::new(AtomicBool::new(false));
    let keep = stop.clone();
    thread::spawn(move || {
        while !keep.load(Ordering::Relaxed) {
            let _ = Command::new("sudo")
                .args(["-n", "-v"])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
            for _ in 0..60 {
                if keep.load(Ordering::Relaxed) {
                    return;
                }
                thread::sleep(Duration::from_secs(1));
            }
        }
    });
    thread::spawn(move || {
        let result = commands.into_iter().try_for_each(|cmd| run_one(cmd, &tx));
        let _ = tx.send(Msg::Finished(result));
    });
    Runner { rx, stop }
}

fn run_one(mut cmd: Command, tx: &Sender<Msg>) -> Result<(), String> {
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("NO_COLOR", "1")
        .env("CARGO_TERM_COLOR", "never")
        .env("TUILNUX_PROGRESS", "1")
        .spawn()
        .map_err(|e| format!("{:?}: {e}", cmd.get_program()))?;
    let readers: Vec<_> = [
        child
            .stdout
            .take()
            .map(|s| Box::new(s) as Box<dyn Read + Send>),
        child
            .stderr
            .take()
            .map(|s| Box::new(s) as Box<dyn Read + Send>),
    ]
    .into_iter()
    .flatten()
    .map(|stream| {
        let tx = tx.clone();
        thread::spawn(move || {
            for line in BufReader::new(stream).split(b'\n').map_while(Result::ok) {
                let text = String::from_utf8_lossy(&line);
                for part in text.split('\r').filter(|p| !p.trim().is_empty()) {
                    let _ = tx.send(Msg::Line(strip_ansi(part)));
                }
            }
        })
    })
    .collect();
    for r in readers {
        let _ = r.join();
    }
    let status = child.wait().map_err(|e| e.to_string())?;
    if status.success() {
        Ok(())
    } else {
        Err(match status.code() {
            Some(c) => format!("Exit-Code {c}"),
            None => "abgebrochen".into(),
        })
    }
}

/// Entfernt Farb- und Steuersequenzen (`ESC [ … Buchstabe`).
pub fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            if chars.peek() == Some(&'[') {
                chars.next();
                for n in chars.by_ref() {
                    if n.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
            continue;
        }
        if c == '\t' {
            out.push_str("    ");
        } else if !c.is_control() {
            out.push(c);
        }
    }
    out
}

/// Sucht den Quellbaum mit `install.sh`: `--root`, sonst neben der Binärdatei (`target/release`).
pub fn find_root(explicit: Option<PathBuf>) -> Option<PathBuf> {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.ancestors().nth(3).map(Path::to_path_buf));
    explicit
        .into_iter()
        .chain(exe_dir)
        .find(|d| d.join("install.sh").is_file() && d.join("Cargo.toml").is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan() -> Plan {
        Plan {
            graphical: true,
            greetd: true,
            enable: false,
            bashrc: true,
            font: true,
            nvidia: false,
            tests: false,
            backend: "auto",
            locale: "de_DE.UTF-8".into(),
            keymap: "de-latin1".into(),
        }
    }

    #[test]
    fn plan_to_install_arguments() {
        assert_eq!(
            plan().args().join(" "),
            "--yes --text --no-enable --no-tests --no-nvidia --backend auto --lang de_DE.UTF-8 --keymap de-latin1"
        );
        let mut p = plan();
        p.graphical = false;
        p.nvidia = true;
        p.locale.clear();
        p.keymap.clear();
        assert_eq!(
            p.args().join(" "),
            "--yes --text --minimal --no-enable --no-tests --nvidia --backend auto"
        );
    }

    #[test]
    fn strips_colors_and_control_characters() {
        assert_eq!(
            strip_ansi("\x1b[1;36m==>\x1b[0m Baue\tjetzt"),
            "==> Baue    jetzt"
        );
        assert!(is_step("##> Installiere"));
        assert!(!is_step("==> Making package: tuilnux"));
        assert!(!is_step("==> Warnung: x"));
        assert!(!is_step("   Compiling x"));
        assert_eq!(display_line("##> Installiere".into()), "==> Installiere");
        assert_eq!(display_line("  -> x".into()), "  -> x");
    }
}
