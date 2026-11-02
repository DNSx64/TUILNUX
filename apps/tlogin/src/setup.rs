//! `tlogin setup` – übernimmt die Anmelde-Einstellungen ins System (als root):
//! `/etc/tuilnux/login.toml`, `/etc/pam.d/greetd` (Schlüssel/Handy-Code) und
//! `/etc/security/faillock.conf` (Sperre nach Fehlversuchen).

use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use tuilnux_config::{
    FactorMode, LoginSettings, Package, PackageManager, faillock_conf, pam_greetd, pam_module_dirs,
};

pub const PAM_FILE: &str = "/etc/pam.d/greetd";
pub const FAILLOCK_FILE: &str = "/etc/security/faillock.conf";
pub const BACKUP_SUFFIX: &str = ".tuilnux-orig";
/// Beschreibbar für den Greeter (letzter Benutzername).
pub const STATE_DIR: &str = "/var/lib/tuilnux/greeter";
pub const LAST_USER_FILE: &str = "last-user";

pub const HELP: &str = "\
tlogin setup – Anmelde-Einstellungen systemweit übernehmen (als root)

Aufruf: sudo tlogin setup [Optionen]
  --from <datei>   Einstellungen aus dieser Datei (Format wie /etc/tuilnux/login.toml);
                   ohne: vorhandene /etc/tuilnux/login.toml erneut anwenden
  --reset          alles zurück auf den Auslieferungszustand
  --yes            fehlende Pakete ohne Rückfrage installieren
  --no-packages    keine Pakete installieren
  --dry-run        nur anzeigen, was geschrieben würde

Geschrieben wird:
  /etc/tuilnux/login.toml        Aussehen und Verhalten von tlogin
  /etc/pam.d/greetd              Sicherheitsschlüssel (pam_u2f) und Handy-Code
                                 (pam_google_authenticator); Original bleibt als
                                 /etc/pam.d/greetd.tuilnux-orig erhalten
  /etc/security/faillock.conf    Sperre nach Fehlversuchen (gilt für alle Anmeldungen)

Benutzer richten ihre Faktoren selbst ein:
  tuilnux key add      Sicherheitsschlüssel (YubiKey, Nitrokey …) koppeln
  tuilnux otp setup    Handy per QR-Code koppeln (Aegis, 2FAS, Google Authenticator …)";

#[derive(Debug, Default)]
pub struct Options {
    pub from: Option<PathBuf>,
    pub reset: bool,
    pub yes: bool,
    pub no_packages: bool,
    pub dry_run: bool,
}

pub fn parse_args(args: &[String]) -> Result<Option<Options>> {
    let mut o = Options::default();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "-h" | "--help" => return Ok(None),
            "--from" => o.from = Some(it.next().context("--from braucht eine Datei")?.into()),
            "--reset" => o.reset = true,
            "--yes" | "-y" => o.yes = true,
            "--no-packages" => o.no_packages = true,
            "--dry-run" => o.dry_run = true,
            other => bail!("unbekannte Option „{other}“ (siehe tlogin setup --help)"),
        }
    }
    Ok(Some(o))
}

pub fn run(args: &[String]) -> Result<()> {
    let Some(opts) = parse_args(args)? else {
        println!("{HELP}");
        return Ok(());
    };
    if !opts.dry_run && !is_root() {
        bail!("bitte als root starten: sudo tlogin setup …");
    }
    let root = Root(PathBuf::from("/"));
    if opts.reset {
        return reset(&root, opts.dry_run);
    }
    let settings = match &opts.from {
        Some(file) => {
            let text = std::fs::read_to_string(file).with_context(|| file.display().to_string())?;
            LoginSettings::parse(&text).with_context(|| file.display().to_string())?
        }
        None => LoginSettings::load_system()?,
    };
    if !opts.no_packages && !opts.dry_run {
        install_missing(&root, &settings, opts.yes)?;
    }
    for line in apply(&root, &settings, opts.dry_run)? {
        println!("{line}");
    }
    Ok(())
}

#[cfg(unix)]
fn is_root() -> bool {
    // SAFETY: geteuid hat keine Vorbedingungen.
    unsafe { libc::geteuid() == 0 }
}

#[cfg(not(unix))]
fn is_root() -> bool {
    false
}

/// Wurzelverzeichnis (in Tests ein Temp-Ordner).
pub struct Root(pub PathBuf);

impl Root {
    fn path(&self, abs: &str) -> PathBuf {
        self.0.join(abs.trim_start_matches('/'))
    }
}

/// Benötigte PAM-Module mit ihrem Paket.
fn modules(s: &LoginSettings) -> Vec<(&'static str, Package)> {
    let mut v = Vec::new();
    if s.key.enabled() {
        v.push(("pam_u2f.so", Package::PamU2f));
    }
    if s.otp.enabled() {
        v.push(("pam_google_authenticator.so", Package::GoogleAuthenticator));
    }
    v
}

fn module_exists(root: &Root, module: &str) -> bool {
    pam_module_dirs(&root.0)
        .iter()
        .any(|d| d.join(module).is_file())
}

fn package_manager(root: &Root) -> PackageManager {
    PackageManager::detect_in(&root.0).unwrap_or(PackageManager::Pacman)
}

fn install_missing(root: &Root, s: &LoginSettings, yes: bool) -> Result<()> {
    let pm = package_manager(root);
    let missing: Vec<&str> = modules(s)
        .into_iter()
        .filter(|(m, _)| !module_exists(root, m))
        .map(|(_, p)| pm.package(p))
        .collect();
    if missing.is_empty() {
        return Ok(());
    }
    let tool = pm.binary();
    println!("Fehlende Pakete: {}", missing.join(" "));
    if !yes {
        print!("Jetzt mit {tool} installieren? [J/n] ");
        std::io::stdout().flush()?;
        let mut answer = String::new();
        std::io::stdin().lock().read_line(&mut answer)?;
        if matches!(answer.trim().to_lowercase().as_str(), "n" | "nein" | "no") {
            bail!("abgebrochen – PAM bleibt unverändert");
        }
    }
    let status = std::process::Command::new(tool)
        .args(pm.install_args())
        .args(&missing)
        .status()
        .with_context(|| format!("{tool} nicht gefunden"))?;
    if !status.success() {
        bail!("{tool} ist fehlgeschlagen – PAM bleibt unverändert");
    }
    Ok(())
}

/// Schreibt alle Dateien; gibt eine Zusammenfassung zurück.
pub fn apply(root: &Root, s: &LoginSettings, dry_run: bool) -> Result<Vec<String>> {
    s.validate()?;
    // Ein fehlendes Modul mit „required“ würde jede Anmeldung verhindern.
    let pm = package_manager(root);
    for (module, package) in modules(s) {
        if !module_exists(root, module) {
            bail!(
                "{module} fehlt ({}) – PAM bleibt unverändert",
                pm.hint(package)
            );
        }
    }
    let mut notes = Vec::new();

    let pam = root.path(PAM_FILE);
    let pam_backup = backup_path(&pam);
    let original = if pam_backup.is_file() {
        read(&pam_backup)?
    } else {
        read(&pam)?
    };
    let new_pam = pam_greetd(&original, s);
    if dry_run {
        println!("--- {PAM_FILE} ---\n{new_pam}");
    } else {
        if !pam_backup.exists() {
            std::fs::copy(&pam, &pam_backup).with_context(|| pam_backup.display().to_string())?;
        }
        write(&pam, &new_pam, 0o644)?;
    }
    notes.push(format!(
        "{PAM_FILE}: Sicherheitsschlüssel {}, Handy-Code {}",
        mode_text(s.key),
        mode_text(s.otp)
    ));

    let faillock = root.path(FAILLOCK_FILE);
    let faillock_backup = backup_path(&faillock);
    let base = if faillock_backup.is_file() {
        read(&faillock_backup)?
    } else if faillock.is_file() {
        read(&faillock)?
    } else {
        String::new()
    };
    let new_faillock = faillock_conf(&base, s);
    if dry_run {
        println!("--- {FAILLOCK_FILE} ---\n{new_faillock}");
    } else {
        if faillock.is_file() && !faillock_backup.exists() {
            std::fs::copy(&faillock, &faillock_backup)?;
        }
        if let Some(dir) = faillock.parent() {
            std::fs::create_dir_all(dir)?;
        }
        write(&faillock, &new_faillock, 0o644)?;
    }
    notes.push(if s.faillock_deny == 0 {
        format!("{FAILLOCK_FILE}: Sperre aus")
    } else {
        format!(
            "{FAILLOCK_FILE}: Sperre nach {} Fehlversuchen für {}",
            s.faillock_deny,
            if s.faillock_unlock == 0 {
                "immer (faillock --reset)".to_string()
            } else {
                format!("{} s", s.faillock_unlock)
            }
        )
    });

    let config = root.path(tuilnux_config::LOGIN_CONFIG_FILE);
    let toml = s.to_toml()?;
    if dry_run {
        println!("--- {} ---\n{toml}", tuilnux_config::LOGIN_CONFIG_FILE);
    } else {
        if let Some(dir) = config.parent() {
            std::fs::create_dir_all(dir)?;
        }
        write(&config, &toml, 0o644)?;
    }
    notes.push(format!(
        "{}: Aussehen/Verhalten von tlogin",
        tuilnux_config::LOGIN_CONFIG_FILE
    ));

    let state = root.path(STATE_DIR);
    if !dry_run {
        if s.remember_user {
            std::fs::create_dir_all(&state)?;
            set_mode(&state, 0o700)?;
            if root.0 == Path::new("/") {
                let _ = std::process::Command::new("chown")
                    .args(["greeter:greeter", STATE_DIR])
                    .status();
            }
        } else {
            let _ = std::fs::remove_file(state.join(LAST_USER_FILE));
        }
    }
    notes.push("Wirkt ab dem nächsten Anmeldebildschirm (abmelden).".into());
    Ok(notes)
}

pub fn reset(root: &Root, dry_run: bool) -> Result<()> {
    for file in [PAM_FILE, FAILLOCK_FILE] {
        let path = root.path(file);
        let backup = backup_path(&path);
        if backup.is_file() {
            println!("{file}: Original wiederhergestellt");
            if !dry_run {
                std::fs::rename(&backup, &path)?;
            }
        }
    }
    let config = root.path(tuilnux_config::LOGIN_CONFIG_FILE);
    if config.is_file() {
        println!("{}: entfernt", tuilnux_config::LOGIN_CONFIG_FILE);
        if !dry_run {
            std::fs::remove_file(&config)?;
        }
    }
    if !dry_run {
        let _ = std::fs::remove_dir_all(root.path(STATE_DIR));
    }
    Ok(())
}

fn mode_text(m: FactorMode) -> &'static str {
    match m {
        FactorMode::Off => "aus",
        FactorMode::Second => "zusätzlich (2FA)",
        FactorMode::Alternative => "statt Passwort",
    }
}

fn backup_path(path: &Path) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(BACKUP_SUFFIX);
    s.into()
}

fn read(path: &Path) -> Result<String> {
    std::fs::read_to_string(path).with_context(|| path.display().to_string())
}

fn write(path: &Path, content: &str, mode: u32) -> Result<()> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp~");
    let tmp = PathBuf::from(tmp);
    std::fs::write(&tmp, content).with_context(|| tmp.display().to_string())?;
    set_mode(&tmp, mode)?;
    std::fs::rename(&tmp, path).with_context(|| path.display().to_string())?;
    Ok(())
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
        .with_context(|| path.display().to_string())
}

#[cfg(not(unix))]
fn set_mode(_path: &Path, _mode: u32) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAM: &str = "#%PAM-1.0\nauth required pam_securetty.so\nauth include system-local-login\naccount include system-local-login\n";

    fn temp_root(name: &str) -> Root {
        let dir = std::env::temp_dir().join(format!("tlogin-setup-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("etc/pam.d")).unwrap();
        std::fs::create_dir_all(dir.join("etc/security")).unwrap();
        std::fs::write(dir.join("etc/pam.d/greetd"), PAM).unwrap();
        std::fs::write(dir.join("etc/security/faillock.conf"), "# deny = 3\n").unwrap();
        Root(dir)
    }

    #[test]
    fn refuses_missing_modules_and_keeps_pam() {
        let root = temp_root("missing");
        let s = LoginSettings {
            key: FactorMode::Second,
            ..Default::default()
        };
        assert!(apply(&root, &s, false).is_err());
        assert_eq!(read(&root.path(PAM_FILE)).unwrap(), PAM);
        let _ = std::fs::remove_dir_all(&root.0);
    }

    #[test]
    fn applies_and_resets() {
        let root = temp_root("apply");
        std::fs::create_dir_all(root.path("/usr/lib/security")).unwrap();
        for m in ["pam_u2f.so", "pam_google_authenticator.so"] {
            std::fs::write(root.path("/usr/lib/security").join(m), "").unwrap();
        }
        let s = LoginSettings {
            key: FactorMode::Second,
            otp: FactorMode::Alternative,
            faillock_deny: 4,
            remember_user: true,
            ..Default::default()
        };
        apply(&root, &s, false).unwrap();
        let pam = read(&root.path(PAM_FILE)).unwrap();
        assert!(
            pam.contains("pam_u2f.so") && pam.contains("sufficient pam_google"),
            "{pam}"
        );
        assert!(
            read(&root.path(FAILLOCK_FILE))
                .unwrap()
                .contains("deny = 4")
        );
        let cfg = read(&root.path(tuilnux_config::LOGIN_CONFIG_FILE)).unwrap();
        assert_eq!(LoginSettings::parse(&cfg).unwrap(), s);
        assert!(root.path(STATE_DIR).is_dir());
        // Zweimal anwenden ändert nichts, Backup bleibt das Original.
        apply(&root, &s, false).unwrap();
        assert_eq!(read(&root.path(PAM_FILE)).unwrap(), pam);
        assert_eq!(read(&backup_path(&root.path(PAM_FILE))).unwrap(), PAM);
        reset(&root, false).unwrap();
        assert_eq!(read(&root.path(PAM_FILE)).unwrap(), PAM);
        assert_eq!(read(&root.path(FAILLOCK_FILE)).unwrap(), "# deny = 3\n");
        assert!(!root.path(tuilnux_config::LOGIN_CONFIG_FILE).exists());
        let _ = std::fs::remove_dir_all(&root.0);
    }

    #[test]
    fn parses_options() {
        let args: Vec<String> = ["--from", "x.toml", "--yes"].map(String::from).to_vec();
        let o = parse_args(&args).unwrap().unwrap();
        assert_eq!(o.from.as_deref(), Some(Path::new("x.toml")));
        assert!(o.yes && !o.reset);
        assert!(parse_args(&["--help".into()]).unwrap().is_none());
        assert!(parse_args(&["--quatsch".into()]).is_err());
    }
}
