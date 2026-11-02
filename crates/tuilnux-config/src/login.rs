//! Anmeldebildschirm (tlogin): Aussehen, Schutz vor Ausprobieren und zweite Faktoren.
//!
//! Die Werte stehen systemweit in `/etc/tuilnux/login.toml`, weil der Greeter vor der
//! Anmeldung läuft. `tsettings` bearbeitet eine Kopie in den Benutzer-Einstellungen
//! (`[login]`), `sudo tlogin setup` übernimmt sie ins System und schreibt die PAM-Datei.

use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::{Choice, parse_hex};

/// Pfad der systemweiten Datei; mit dieser Variable überschreibbar (Tests, Demo).
pub const LOGIN_CONFIG_ENV: &str = "TUILNUX_LOGIN_CONFIG";
pub const LOGIN_CONFIG_FILE: &str = "/etc/tuilnux/login.toml";
/// PAM-Ursprung für Sicherheitsschlüssel (pam_u2f/pamu2fcfg).
pub const U2F_ORIGIN: &str = "pam://tuilnux";
/// Benutzerdatei für Sicherheitsschlüssel (Standard von pam_u2f).
pub const U2F_KEYS_FILE: &str = ".config/Yubico/u2f_keys";
/// Benutzerdatei für Handy-Codes (pam_google_authenticator).
pub const OTP_FILE: &str = ".google_authenticator";

choice!(
    /// Schriftzug über der Anmeldebox.
    LogoStyle [Thin] {
        Thin = "thin", "dünn (Linien)";
        Bold = "bold", "fett";
        Small = "small", "klein";
        None = "none", "ohne";
    }
);

choice!(
    /// Wie das Passwort beim Tippen erscheint.
    SecretDisplay [Dots] {
        Dots = "dots", "Punkte (•••)";
        Mask = "mask", "feste Länge (verrät nichts)";
        Hidden = "hidden", "unsichtbar";
    }
);

choice!(
    /// Wie ein zusätzlicher Anmeldeweg (Schlüssel, Handy-Code) eingebunden ist.
    FactorMode [Off] {
        Off = "off", "aus";
        Second = "second", "zusätzlich (2FA)";
        Alternative = "alternative", "statt Passwort";
    }
);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LoginSettings {
    /// Sprache des Anmeldebildschirms (`auto` = `/etc/locale.conf`).
    pub language: String,
    /// Linien- und Textfarbe.
    pub accent: String,
    pub background: String,
    pub logo: LogoStyle,
    /// Rechnername im Titel (`user@arch:~$`).
    pub show_host: bool,
    pub show_clock: bool,
    /// Text unter der Box, z. B. Hinweis für Fremde.
    pub notice: String,
    pub secret_display: SecretDisplay,
    /// Warnung bei aktivem Feststelltaste.
    pub caps_warning: bool,
    /// Benutzername vorausfüllen (letzte erfolgreiche Anmeldung).
    pub remember_user: bool,
    /// Nach so vielen Fehlversuchen wartet tlogin (0 = nie).
    pub max_attempts: u32,
    /// Wartezeit danach; verdoppelt sich bei jeder weiteren Sperre.
    pub lockout_seconds: u32,
    /// Eingaben nach so vielen Sekunden ohne Taste löschen (0 = nie).
    pub idle_timeout: u32,
    /// reboot/poweroff am Anmeldebildschirm.
    pub allow_power: bool,
    /// F2 wechselt die Sitzung (TUILNUX, Terminal, Bash).
    pub allow_session_switch: bool,
    /// Systemweite Sperre (pam_faillock): Fehlversuche bis zur Sperre, 0 = aus.
    pub faillock_deny: u32,
    /// Sperrdauer von pam_faillock in Sekunden (0 = bis root entsperrt).
    pub faillock_unlock: u32,
    /// Sicherheitsschlüssel (FIDO2/U2F, z. B. YubiKey) über pam_u2f.
    pub key: FactorMode,
    /// Code vom Handy (per QR-Code gekoppelt) über pam_google_authenticator.
    pub otp: FactorMode,
}

impl Default for LoginSettings {
    fn default() -> Self {
        Self {
            language: "auto".into(),
            accent: "#22c8d6".into(),
            background: "#000000".into(),
            logo: LogoStyle::Thin,
            show_host: true,
            show_clock: true,
            notice: String::new(),
            secret_display: SecretDisplay::Dots,
            caps_warning: true,
            remember_user: false,
            max_attempts: 3,
            lockout_seconds: 30,
            idle_timeout: 120,
            allow_power: true,
            allow_session_switch: true,
            faillock_deny: 5,
            faillock_unlock: 600,
            key: FactorMode::Off,
            otp: FactorMode::Off,
        }
    }
}

impl LoginSettings {
    pub fn validate(&self) -> Result<()> {
        for (name, value) in [
            ("login.accent", &self.accent),
            ("login.background", &self.background),
        ] {
            if parse_hex(value).is_none() {
                bail!("{name} = \"{value}\" ist keine Farbe im Format #rrggbb");
            }
        }
        if self.notice.chars().count() > 300 || self.notice.chars().any(|c| c.is_control()) {
            bail!("login.notice: höchstens 300 Zeichen, keine Steuerzeichen");
        }
        if self.max_attempts > 100 {
            bail!("login.max_attempts: höchstens 100");
        }
        if self.lockout_seconds > 3600 {
            bail!("login.lockout_seconds: höchstens 3600");
        }
        if self.idle_timeout != 0 && !(10..=3600).contains(&self.idle_timeout) {
            bail!("login.idle_timeout: 0 (aus) oder 10–3600 Sekunden");
        }
        if self.faillock_deny > 100 || self.faillock_unlock > 86_400 {
            bail!("login.faillock_*: deny ≤ 100, unlock ≤ 86400");
        }
        if self.key == FactorMode::Alternative && self.otp == FactorMode::Alternative {
            bail!("Sicherheitsschlüssel und Handy-Code können nicht beide „statt Passwort“ sein");
        }
        Ok(())
    }

    pub fn file() -> PathBuf {
        std::env::var_os(LOGIN_CONFIG_ENV)
            .filter(|p| !p.is_empty())
            .map_or_else(|| PathBuf::from(LOGIN_CONFIG_FILE), PathBuf::from)
    }

    pub fn parse(s: &str) -> Result<Self> {
        let settings: Self = toml::from_str(s)?;
        settings.validate()?;
        Ok(settings)
    }

    /// Systemweite Einstellungen; fehlt die Datei oder ist sie kaputt, gelten die Standards.
    pub fn load_system() -> Result<Self> {
        let file = Self::file();
        match std::fs::read_to_string(&file) {
            Ok(s) => Self::parse(&s).with_context(|| file.display().to_string()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e).with_context(|| file.display().to_string()),
        }
    }

    pub fn to_toml(&self) -> Result<String> {
        Ok(format!(
            "# TUILNUX-Anmeldebildschirm (tlogin) – erzeugt von `tlogin setup`.\n\
             # Ändern: tsettings → Anmeldung → Systemweit übernehmen\n\n{}",
            toml::to_string(self)?
        ))
    }

    /// Wird nach dem Benutzernamen zuerst nach Schlüssel/Code gefragt?
    pub fn passwordless(&self) -> bool {
        self.key == FactorMode::Alternative || self.otp == FactorMode::Alternative
    }
}

fn u2f_line(control: &str, nouserok: bool) -> String {
    format!(
        "auth       {control:<10} pam_u2f.so cue{} origin={U2F_ORIGIN} appid={U2F_ORIGIN}",
        if nouserok { " nouserok" } else { "" }
    )
}

fn otp_line(control: &str, nullok: bool) -> String {
    format!(
        "auth       {control:<10} pam_google_authenticator.so{}",
        if nullok { " nullok" } else { "" }
    )
}

pub const PAM_MARK: &str = "# TUILNUX";

/// Baut `/etc/pam.d/greetd` aus dem Original und den gewählten Faktoren.
///
/// - „statt Passwort“: `sufficient` vor dem Passwort-Stapel – gelingt der Schlüssel/Code,
///   ist man angemeldet, sonst folgt das Passwort. Ohne `nouserok`/`nullok`, sonst
///   käme jeder ohne eingerichteten Faktor durch.
/// - „zusätzlich“: `required` nach dem Passwort; Benutzer ohne eingerichteten Faktor
///   melden sich weiter nur mit Passwort an (`nouserok`/`nullok`).
pub fn pam_greetd(original: &str, s: &LoginSettings) -> String {
    let mut before = Vec::new();
    let mut after = Vec::new();
    match s.key {
        FactorMode::Off => {}
        FactorMode::Alternative => before.push(u2f_line("sufficient", false)),
        FactorMode::Second => after.push(u2f_line("required", true)),
    }
    match s.otp {
        FactorMode::Off => {}
        FactorMode::Alternative => before.push(otp_line("sufficient", false)),
        FactorMode::Second => after.push(otp_line("required", true)),
    }
    let lines: Vec<&str> = original
        .lines()
        .filter(|l| !l.trim_end().ends_with(PAM_MARK))
        .collect();
    // Debian/Ubuntu: `@include common-auth` statt `auth include …`.
    let debian_auth = |l: &&str| {
        let mut w = l.split_whitespace();
        w.next() == Some("@include") && w.next().is_some_and(|f| f.ends_with("-auth"))
    };
    let is_auth = |l: &&str| {
        l.split_whitespace()
            .next()
            .map(|t| t.trim_start_matches('-'))
            == Some("auth")
            || debian_auth(l)
    };
    let is_stack = |l: &&str| {
        debian_auth(l)
            || (is_auth(l)
                && l.split_whitespace()
                    .nth(1)
                    .is_some_and(|c| c == "include" || c == "substack"))
    };
    let first = lines
        .iter()
        .position(is_stack)
        .or_else(|| lines.iter().rposition(is_auth).map(|i| i + 1))
        .unwrap_or(lines.len());
    let last = lines
        .iter()
        .rposition(is_auth)
        .map_or(first, |i| (i + 1).max(first));
    let tag = |v: Vec<String>| -> Vec<String> {
        v.into_iter().map(|l| format!("{l}  {PAM_MARK}")).collect()
    };
    let mut out: Vec<String> = Vec::new();
    out.extend(lines[..first].iter().map(|l| l.to_string()));
    out.extend(tag(before));
    out.extend(lines[first..last].iter().map(|l| l.to_string()));
    out.extend(tag(after));
    out.extend(lines[last..].iter().map(|l| l.to_string()));
    let mut text = out.join("\n");
    text.push('\n');
    text
}

/// Setzt `deny` und `unlock_time` in `/etc/security/faillock.conf`.
pub fn faillock_conf(original: &str, s: &LoginSettings) -> String {
    let mut out = Vec::new();
    let key_of = |l: &str| {
        if !l.contains('=') {
            return String::new();
        }
        let t = l.trim_start_matches(['#', ' ']).trim();
        t.split(['=', ' ']).next().unwrap_or("").to_string()
    };
    let mut seen_deny = false;
    let mut seen_unlock = false;
    for line in original.lines() {
        match key_of(line).as_str() {
            "deny" if !seen_deny => {
                seen_deny = true;
                out.push(faillock_value("deny", s.faillock_deny));
            }
            "unlock_time" if !seen_unlock => {
                seen_unlock = true;
                out.push(faillock_value("unlock_time", s.faillock_unlock));
            }
            "deny" | "unlock_time" => {}
            _ => out.push(line.to_string()),
        }
    }
    if !seen_deny {
        out.push(faillock_value("deny", s.faillock_deny));
    }
    if !seen_unlock {
        out.push(faillock_value("unlock_time", s.faillock_unlock));
    }
    let mut text = out.join("\n");
    text.push('\n');
    text
}

fn faillock_value(key: &str, value: u32) -> String {
    // deny = 0 schaltet die Sperre bei pam_faillock aus.
    format!("{key} = {value}")
}

/// Anzahl der Sicherheitsschlüssel in einer `u2f_keys`-Zeile (`user:key1:key2…`).
pub fn u2f_key_count(content: &str, user: &str) -> usize {
    content
        .lines()
        .find_map(|l| l.strip_prefix(user)?.strip_prefix(':'))
        .map_or(0, |rest| rest.split(':').filter(|k| !k.is_empty()).count())
}

impl FactorMode {
    pub fn enabled(self) -> bool {
        self != Self::Off
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ARCH: &str = "#%PAM-1.0\n\
        auth       required     pam_securetty.so\n\
        auth       requisite    pam_nologin.so\n\
        auth       include      system-local-login\n\
        account    include      system-local-login\n\
        session    include      system-local-login\n";

    #[test]
    fn defaults_are_valid_and_roundtrip() {
        let d = LoginSettings::default();
        d.validate().unwrap();
        let back = LoginSettings::parse(&d.to_toml().unwrap()).unwrap();
        assert_eq!(back, d);
        assert!(LoginSettings::parse("accent = \"blau\"").is_err());
        assert!(LoginSettings::parse("idle_timeout = 3").is_err());
        assert!(LoginSettings::parse("key = \"alternative\"\notp = \"alternative\"").is_err());
        assert!(LoginSettings::parse("unbekannt = 1").is_err());
    }

    #[test]
    fn pam_off_keeps_original() {
        assert_eq!(pam_greetd(ARCH, &LoginSettings::default()), ARCH);
    }

    #[test]
    fn pam_second_factor_after_password() {
        let s = LoginSettings {
            key: FactorMode::Second,
            otp: FactorMode::Second,
            ..Default::default()
        };
        let pam = pam_greetd(ARCH, &s);
        let lines: Vec<&str> = pam.lines().collect();
        let include = lines
            .iter()
            .position(|l| l.contains("include      system-local-login"))
            .unwrap();
        let u2f = lines.iter().position(|l| l.contains("pam_u2f.so")).unwrap();
        let otp = lines
            .iter()
            .position(|l| l.contains("pam_google_authenticator"))
            .unwrap();
        assert!(u2f > include && otp > include, "{pam}");
        assert!(lines[u2f].contains("required") && lines[u2f].contains("nouserok"));
        assert!(lines[otp].contains("nullok"));
        assert!(lines[u2f + 2].starts_with("account"), "{pam}");
        // Erneut anwenden ersetzt statt zu verdoppeln; „aus“ stellt das Original her.
        assert_eq!(pam_greetd(&pam, &s), pam);
        assert_eq!(pam_greetd(&pam, &LoginSettings::default()), ARCH);
    }

    #[test]
    fn pam_alternative_is_never_nullok() {
        let s = LoginSettings {
            key: FactorMode::Alternative,
            otp: FactorMode::Second,
            ..Default::default()
        };
        let pam = pam_greetd(ARCH, &s);
        let lines: Vec<&str> = pam.lines().collect();
        let include = lines
            .iter()
            .position(|l| l.contains("auth       include"))
            .unwrap();
        let u2f = lines.iter().position(|l| l.contains("pam_u2f.so")).unwrap();
        assert!(u2f < include && lines[u2f].contains("sufficient"));
        assert!(!lines[u2f].contains("nouserok"), "{pam}");
        let s = LoginSettings {
            otp: FactorMode::Alternative,
            ..Default::default()
        };
        let pam = pam_greetd(ARCH, &s);
        assert!(
            pam.contains("sufficient pam_google_authenticator.so  # TUILNUX"),
            "{pam}"
        );
        assert!(!pam.contains("nullok"));
    }

    #[test]
    fn pam_debian_common_auth() {
        const DEBIAN: &str = "#%PAM-1.0\n\
            auth       requisite    pam_nologin.so\n\
            @include common-auth\n\
            -auth      optional     pam_gnome_keyring.so\n\
            @include common-account\n\
            session    required     pam_limits.so\n\
            @include common-session\n";
        let s = LoginSettings {
            key: FactorMode::Alternative,
            otp: FactorMode::Second,
            ..Default::default()
        };
        let pam = pam_greetd(DEBIAN, &s);
        let lines: Vec<&str> = pam.lines().collect();
        let pos = |needle: &str| lines.iter().position(|l| l.contains(needle)).unwrap();
        assert!(pos("pam_u2f.so") < pos("@include common-auth"), "{pam}");
        assert!(
            pos("pam_google_authenticator") > pos("pam_gnome_keyring"),
            "{pam}"
        );
        assert!(
            pos("pam_google_authenticator") < pos("@include common-account"),
            "{pam}"
        );
        assert_eq!(pam_greetd(&pam, &LoginSettings::default()), DEBIAN);
    }

    #[test]
    fn faillock_values_are_set_once() {
        let orig = "# deny = 3\n#\n# unlock_time = 600\nsilent\n";
        let s = LoginSettings {
            faillock_deny: 4,
            faillock_unlock: 120,
            ..Default::default()
        };
        let out = faillock_conf(orig, &s);
        assert_eq!(out, "deny = 4\n#\nunlock_time = 120\nsilent\n");
        assert_eq!(faillock_conf(&out, &s), out);
        assert_eq!(
            faillock_conf("silent\n", &s),
            "silent\ndeny = 4\nunlock_time = 120\n"
        );
    }

    #[test]
    fn counts_u2f_keys() {
        let c = "anna:a,b,es256,+presence\nuser:k1,x,es256,+presence:k2,y,es256,+presence\n";
        assert_eq!(u2f_key_count(c, "user"), 2);
        assert_eq!(u2f_key_count(c, "anna"), 1);
        assert_eq!(u2f_key_count(c, "us"), 0);
        assert_eq!(u2f_key_count(c, "bob"), 0);
    }
}
