//! tuilnux – die Desktop-Sitzung.
//!
//! Ohne Argumente startet die Sitzung: Das Vollbild-Terminal ist der Desktop, unten
//! sitzt die Statusleiste mit den Tasks, Programme öffnen sich als verschiebbare
//! Fenster darüber. Mit Argumenten steuert `tuilnux` eine laufende Sitzung.

mod account;
mod apps;
mod cli;
mod config;
mod draw;
mod gui;
mod search;
mod session;

use anyhow::{Result, bail};
use tuilnux_ipc::SOCK_ENV;

const HELP: &str = "\
tuilnux – TUILNUX-Desktop: Terminal als Desktop, Fenster, Taskleiste, Taskmanager

Sitzung:
  tuilnux                       Sitzung starten (von tlogin/greetd oder im TTY)
  tuilnux --force               auch innerhalb einer Sitzung starten

Programme suchen (Super+F, im TTY: Präfix, dann a):
  tuilnux search [TEXT]         installierte Programme suchen und starten
  tuilnux search --list [TEXT]  Treffer nur ausgeben

Steuern (aus einer Shell in der Sitzung):
  tuilnux open [-t TITEL] [-s 100x30] [--] BEFEHL …
                                Programm als Fenster öffnen, gibt die Nummer aus
  tuilnux list                  Tasks mit PID, CPU und RAM
  tuilnux focus NR              Fenster nach vorne holen (0 = Desktop)
  tuilnux tab [NAME|NR]         Task per Programmname/Titel wiederherstellen und
                                fokussieren, ohne NAME: Tasks auflisten
                                (in der Shell kurz: tab ttop)
  tuilnux launch [--] BEFEHL …  Programm im Hintergrund starten (eigenes Terminal
                                ohne Fenster); grafische Programme startet die
                                Shell automatisch so (tsettings → Shell)
  tuilnux tab --shell [NAME]    Terminal/Ausgabe eines Hintergrundprogramms zeigen
                                (Fenster schließen = läuft weiter)
  tuilnux close NR              Fenster schließen (Hintergrundprogramm: beenden)
  tuilnux min NR | max NR       minimieren / maximieren
  tuilnux notify TEXT           Meldung in der Statusleiste
  tuilnux reload                Einstellungen (tuilnux.toml) neu laden
  tuilnux logout                Sitzung beenden

Tasten (Präfix, Standard Strg+Leertaste, danach):
  Tab  1-9  0     Fenster wechseln, Desktop
  Enter / n       neues Terminal-Fenster
  a               Programme suchen (ohne Präfix: Super+F)
  t               Taskmanager      ?  Hilfe
  x  m  f         schließen, minimieren, maximieren
  ←↑→↓ / Shift    verschieben / Größe ändern
  q               abmelden

Ohne Präfix:
  Strg+Alt+1-9    Fenster wiederherstellen und fokussieren (Strg+Alt+0 = Desktop)

Maus: Titel ziehen = verschieben, Rand/Ecken ziehen = Größe, _ □ x
Grafische Programme (Firefox …) stehen ebenfalls in der Taskleiste und bei tab;
Super+Pfeil runter minimiert sie, ein Klick in der Taskleiste holt sie zurück.
Tipp für ~/.bashrc:
  alias open='tuilnux open --'

Ohne Sitzung:
  tuilnux shell-init [bash]     Shell-Skript aus den Einstellungen (Prompt/PS1,
                                Begrüßung, Aliase …), geladen von bashrc.sh:
                                eval \"$(tuilnux shell-init bash)\"
  tuilnux outputs               Bildschirme (wlr-randr) anzeigen
  tuilnux outputs --apply       Bildschirm-Einstellungen anwenden

Konto und Anmeldung:
  tuilnux otp setup|status|remove
                                Handy per QR-Code als zweiten Faktor koppeln
  tuilnux key add|list|remove   Sicherheitsschlüssel (YubiKey …) koppeln
  sudo tlogin setup             Anmelde-Einstellungen systemweit übernehmen

Einstellungen: tsettings (oder ~/.config/tuilnux/tuilnux.toml von Hand)
";

fn main() {
    if let Err(e) = real_main() {
        eprintln!("tuilnux: {e:#}");
        std::process::exit(1);
    }
}

fn real_main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let first = args.first().map(String::as_str);
    match first {
        Some("-h" | "--help" | "help") => {
            print!("{HELP}");
            Ok(())
        }
        Some("-V" | "--version") => {
            println!("tuilnux {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Some(cmd) if cli::CLIENT_COMMANDS.contains(&cmd) => cli::run(cmd, &args[1..]),
        Some("shell-init") => {
            match args.get(1).map(String::as_str) {
                None | Some("bash") => {}
                Some(other) => bail!("shell-init: nur bash wird unterstützt, nicht „{other}“"),
            }
            let settings = tuilnux_config::Settings::load_or_default();
            let apps = tuilnux_config::gui_programs(&settings.bash);
            print!("{}", tuilnux_config::bash_init(&settings, &apps));
            Ok(())
        }
        Some("search") => search::run(&args[1..]),
        Some("outputs") => outputs(args.get(1).map(String::as_str)),
        Some("otp") => account::otp(&args[1..]),
        Some("key") => account::key(&args[1..]),
        None | Some("--force" | "session") => {
            let force = args.iter().any(|a| a == "--force");
            if !force && std::env::var_os(SOCK_ENV).is_some() {
                bail!(
                    "Du bist bereits in einer TUILNUX-Sitzung.\n\
                     Programme als Fenster öffnen: tuilnux open -- BEFEHL\n\
                     Trotzdem verschachtelt starten: tuilnux --force"
                );
            }
            let config = config::load()?;
            session::run(config)
        }
        Some(other) => bail!("unbekannter Befehl „{other}“ – siehe tuilnux help"),
    }
}

fn outputs(arg: Option<&str>) -> Result<()> {
    let settings = tuilnux_config::Settings::load()?;
    match arg {
        None => {
            for o in tuilnux_config::detect_outputs()? {
                let mark = if o.name == settings.displays.desktop {
                    "  (Desktop)"
                } else {
                    ""
                };
                println!("{:<12} {}{mark}", o.name, o.summary());
            }
            Ok(())
        }
        Some("--apply") => {
            match tuilnux_config::apply_outputs(&settings.displays, true)? {
                Some(note) => println!("{note}"),
                None => println!("keine Bildschirme erkannt"),
            }
            Ok(())
        }
        Some(other) => bail!("outputs: unbekannte Option „{other}“"),
    }
}
