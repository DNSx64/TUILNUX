//! Konto-Befehle: Handy per QR-Code koppeln (`tuilnux otp`) und
//! Sicherheitsschlüssel verwalten (`tuilnux key`). Die Anmeldung prüft beides
//! über PAM (pam_google_authenticator / pam_u2f), eingeschaltet mit `tlogin setup`.

use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use tuilnux_config::{OTP_FILE, U2F_KEYS_FILE, U2F_ORIGIN, u2f_key_count};

pub const OTP_HELP: &str = "\
tuilnux otp – Handy als zweiten Faktor koppeln (QR-Code)

  tuilnux otp setup [--force]  QR-Code anzeigen, mit einer Authenticator-App
                               (Aegis, 2FAS, Google Authenticator …) scannen,
                               Code bestätigen; schreibt ~/.google_authenticator
  tuilnux otp status           gekoppelt?
  tuilnux otp remove           Kopplung löschen

Am Anmeldebildschirm fragt tlogin dann nach dem 6-stelligen Code
(systemweit einschalten: tsettings → Anmeldung, oder sudo tlogin setup).";

pub const KEY_HELP: &str = "\
tuilnux key – Sicherheitsschlüssel (FIDO2/U2F, z. B. YubiKey) verwalten

  tuilnux key add      Schlüssel einstecken, berühren – wird gekoppelt
  tuilnux key list     Anzahl gekoppelter Schlüssel
  tuilnux key remove   alle Schlüssel dieses Benutzers entfernen

Braucht das Paket pam-u2f (pamu2fcfg). Datei: ~/.config/Yubico/u2f_keys";

fn home() -> Result<PathBuf> {
    std::env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .map(PathBuf::from)
        .context("HOME ist nicht gesetzt")
}

fn username() -> String {
    std::env::var("USER")
        .or_else(|_| std::env::var("LOGNAME"))
        .unwrap_or_else(|_| "user".into())
}

fn hostname() -> String {
    std::fs::read_to_string("/etc/hostname")
        .map(|s| s.trim().to_string())
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "arch".into())
}

fn ask(question: &str) -> Result<String> {
    print!("{question}");
    std::io::stdout().flush()?;
    let mut line = String::new();
    std::io::stdin().lock().read_line(&mut line)?;
    Ok(line.trim().to_string())
}

fn yes(answer: &str) -> bool {
    matches!(answer.to_lowercase().as_str(), "j" | "ja" | "y" | "yes")
}

/// Schreibt eine Datei nur für den Benutzer lesbar (0600), atomar.
fn write_private(path: &Path, content: &str) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp~");
    let tmp = PathBuf::from(tmp);
    let _ = std::fs::remove_file(&tmp);
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(&tmp)
        .with_context(|| tmp.display().to_string())?;
    file.write_all(content.as_bytes())?;
    file.sync_all()?;
    std::fs::rename(&tmp, path).with_context(|| path.display().to_string())?;
    Ok(())
}

fn random_bytes(n: usize) -> Result<Vec<u8>> {
    use std::io::Read;
    let mut buf = vec![0; n];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut buf))
        .context("/dev/urandom nicht lesbar")?;
    Ok(buf)
}

// --- TOTP (RFC 6238) ---------------------------------------------------------

/// SHA-1 (RFC 3174) – nur für HMAC-SHA1 bei TOTP.
fn sha1(data: &[u8]) -> [u8; 20] {
    let mut h: [u32; 5] = [
        0x6745_2301,
        0xEFCD_AB89,
        0x98BA_DCFE,
        0x1032_5476,
        0xC3D2_E1F0,
    ];
    let mut msg = data.to_vec();
    let bits = (data.len() as u64).wrapping_mul(8);
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bits.to_be_bytes());
    for chunk in msg.chunks(64) {
        let mut w = [0u32; 80];
        for (i, word) in chunk.chunks(4).enumerate() {
            w[i] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let [mut a, mut b, mut c, mut d, mut e] = h;
        for (i, wi) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((b & c) | (!b & d), 0x5A82_7999),
                20..=39 => (b ^ c ^ d, 0x6ED9_EBA1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1B_BCDC),
                _ => (b ^ c ^ d, 0xCA62_C1D6),
            };
            let t = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(*wi);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = t;
        }
        for (x, y) in h.iter_mut().zip([a, b, c, d, e]) {
            *x = x.wrapping_add(y);
        }
    }
    let mut out = [0u8; 20];
    for (i, v) in h.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&v.to_be_bytes());
    }
    out
}

fn hmac_sha1(key: &[u8], msg: &[u8]) -> [u8; 20] {
    let mut k = [0u8; 64];
    if key.len() > 64 {
        k[..20].copy_from_slice(&sha1(key));
    } else {
        k[..key.len()].copy_from_slice(key);
    }
    let mut inner: Vec<u8> = k.iter().map(|b| b ^ 0x36).collect();
    inner.extend_from_slice(msg);
    let mut outer: Vec<u8> = k.iter().map(|b| b ^ 0x5c).collect();
    outer.extend_from_slice(&sha1(&inner));
    sha1(&outer)
}

/// HOTP-Wert (RFC 4226) mit `digits` Stellen.
pub fn hotp(secret: &[u8], counter: u64, digits: u32) -> u32 {
    let mac = hmac_sha1(secret, &counter.to_be_bytes());
    let o = (mac[19] & 0x0f) as usize;
    let bin = u32::from_be_bytes([mac[o] & 0x7f, mac[o + 1], mac[o + 2], mac[o + 3]]);
    bin % 10u32.pow(digits)
}

/// Prüft einen 6-stelligen Code mit ±1 Zeitfenster (30 s).
pub fn totp_matches(secret: &[u8], unix_time: u64, code: &str) -> bool {
    let Ok(code) = code.trim().replace(' ', "").parse::<u32>() else {
        return false;
    };
    let step = unix_time / 30;
    [step.saturating_sub(1), step, step + 1]
        .iter()
        .any(|&s| hotp(secret, s, 6) == code)
}

const BASE32: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

pub fn base32(data: &[u8]) -> String {
    let mut out = String::new();
    let (mut buffer, mut bits) = (0u32, 0u32);
    for &b in data {
        buffer = (buffer << 8) | u32::from(b);
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out.push(BASE32[((buffer >> bits) & 31) as usize] as char);
        }
    }
    if bits > 0 {
        out.push(BASE32[((buffer << (5 - bits)) & 31) as usize] as char);
    }
    out
}

fn url_encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

pub fn otpauth_uri(secret_b32: &str, user: &str, host: &str) -> String {
    let label = url_encode(&format!("TUILNUX:{user}@{host}"));
    format!(
        "otpauth://totp/{label}?secret={secret_b32}&issuer=TUILNUX&algorithm=SHA1&digits=6&period=30"
    )
}

/// QR-Code aus Halbblöcken (zwei Module pro Zeichen), schwarz auf weiß mit Ruhezone.
pub fn qr_lines(data: &str) -> Result<Vec<String>> {
    let code = qrcode::QrCode::new(data.as_bytes()).context("QR-Code zu groß")?;
    let w = code.width();
    let colors = code.to_colors();
    const QUIET: usize = 2;
    let size = w + 2 * QUIET;
    let dark = |x: usize, y: usize| -> bool {
        if x < QUIET || y < QUIET || x >= w + QUIET || y >= w + QUIET {
            return false;
        }
        colors[(y - QUIET) * w + (x - QUIET)] == qrcode::Color::Dark
    };
    let mut lines = Vec::new();
    for y in (0..size).step_by(2) {
        // Vordergrund weiß (helle Module), Hintergrund schwarz (dunkle Module).
        let mut line = String::from("\x1b[97;40m");
        for x in 0..size {
            let top = !dark(x, y);
            let bottom = y + 1 < size && !dark(x, y + 1);
            line.push(match (top, bottom || y + 1 >= size) {
                (true, true) => '█',
                (true, false) => '▀',
                (false, true) => '▄',
                (false, false) => ' ',
            });
        }
        line.push_str("\x1b[0m");
        lines.push(line);
    }
    Ok(lines)
}

/// Inhalt von `~/.google_authenticator` (Format von google-authenticator).
pub fn ga_file(secret_b32: &str, scratch: &[u32]) -> String {
    let mut s = format!(
        "{secret_b32}\n\" RATE_LIMIT 3 30\n\" WINDOW_SIZE 3\n\" DISALLOW_REUSE\n\" TOTP_AUTH\n"
    );
    for code in scratch {
        s.push_str(&format!("{code:08}\n"));
    }
    s
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

pub fn otp(args: &[String]) -> Result<()> {
    let file = home()?.join(OTP_FILE);
    match args.first().map(String::as_str) {
        None | Some("status") => {
            if file.is_file() {
                println!("Handy gekoppelt ({})", file.display());
            } else {
                println!("Kein Handy gekoppelt – tuilnux otp setup");
            }
            Ok(())
        }
        Some("remove") => {
            if !file.is_file() {
                println!("Kein Handy gekoppelt.");
                return Ok(());
            }
            if !yes(&ask("Kopplung wirklich löschen? [j/N] ")?) {
                println!("Abgebrochen.");
                return Ok(());
            }
            std::fs::remove_file(&file)?;
            println!("Kopplung gelöscht. Den Eintrag „TUILNUX“ in der App kannst du entfernen.");
            Ok(())
        }
        Some("setup") => {
            let force = args.iter().any(|a| a == "--force");
            if file.exists()
                && !force
                && !yes(&ask(
                    "Es ist schon ein Handy gekoppelt. Neu koppeln? [j/N] ",
                )?)
            {
                println!("Abgebrochen.");
                return Ok(());
            }
            otp_setup(&file)
        }
        Some("-h" | "--help" | "help") => {
            println!("{OTP_HELP}");
            Ok(())
        }
        Some(other) => bail!("otp: unbekannt „{other}“ – siehe tuilnux otp help"),
    }
}

fn otp_setup(file: &Path) -> Result<()> {
    let secret = random_bytes(20)?;
    let b32 = base32(&secret);
    let uri = otpauth_uri(&b32, &username(), &hostname());
    println!("Scanne den QR-Code mit deiner Authenticator-App:\n");
    for line in qr_lines(&uri)? {
        println!("  {line}");
    }
    println!("\nOder von Hand eingeben: {b32}  (zeitbasiert, 6 Ziffern, 30 s)\n");
    for attempt in 1..=3 {
        let code = ask("Code aus der App (leer = abbrechen): ")?;
        if code.is_empty() {
            println!("Abgebrochen – nichts geändert.");
            return Ok(());
        }
        if totp_matches(&secret, now(), &code) {
            let scratch: Vec<u32> = random_bytes(20)?
                .chunks(4)
                .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]) % 90_000_000 + 10_000_000)
                .collect();
            write_private(file, &ga_file(&b32, &scratch))?;
            println!("\nGekoppelt! Notfall-Codes (je einmal gültig, sicher aufbewahren):");
            for c in &scratch {
                println!("  {c:08}");
            }
            println!(
                "\nAm Anmeldebildschirm wirksam, sobald der Handy-Code systemweit an ist\n\
                 (tsettings → Anmeldung → Handy-Code, dann „Systemweit übernehmen“)."
            );
            return Ok(());
        }
        println!("Code passt nicht (Versuch {attempt}/3) – stimmt die Uhrzeit am Handy?");
    }
    bail!("Kopplung fehlgeschlagen – nichts geändert")
}

// --- Sicherheitsschlüssel ----------------------------------------------------

/// Hängt die Ausgabe von `pamu2fcfg` an die Zeile des Benutzers an.
pub fn merge_u2f(content: &str, user: &str, output: &str) -> String {
    let output = output.trim();
    let entry = output
        .strip_prefix(&format!("{user}:"))
        .or_else(|| output.strip_prefix(':'))
        .unwrap_or(output);
    let mut found = false;
    let mut lines: Vec<String> = content
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            if l.strip_prefix(user).is_some_and(|r| r.starts_with(':')) {
                found = true;
                format!("{l}:{entry}")
            } else {
                l.to_string()
            }
        })
        .collect();
    if !found {
        lines.push(format!("{user}:{entry}"));
    }
    lines.join("\n") + "\n"
}

pub fn key(args: &[String]) -> Result<()> {
    let file = home()?.join(U2F_KEYS_FILE);
    let user = username();
    let content = std::fs::read_to_string(&file).unwrap_or_default();
    match args.first().map(String::as_str) {
        None | Some("list") => {
            let n = u2f_key_count(&content, &user);
            println!("{n} Sicherheitsschlüssel gekoppelt ({})", file.display());
            Ok(())
        }
        Some("add") => {
            println!("Schlüssel einstecken und berühren, wenn er blinkt …");
            let output = std::process::Command::new("pamu2fcfg")
                .args(["-u", &user, "-o", U2F_ORIGIN, "-i", U2F_ORIGIN])
                .stderr(std::process::Stdio::inherit())
                .output();
            let output = match output {
                Ok(o) => o,
                Err(_) => {
                    bail!(
                        "pamu2fcfg fehlt – {} (oder sudo tlogin setup)",
                        tuilnux_config::install_hint(tuilnux_config::Package::PamU2f)
                    )
                }
            };
            if !output.status.success() {
                bail!("Kein Schlüssel gekoppelt (pamu2fcfg ist fehlgeschlagen)");
            }
            let text = String::from_utf8_lossy(&output.stdout);
            write_private(&file, &merge_u2f(&content, &user, &text))?;
            println!(
                "Gekoppelt – jetzt {} Schlüssel. Wirksam, sobald der Sicherheitsschlüssel\n\
                 systemweit an ist (tsettings → Anmeldung).",
                u2f_key_count(&std::fs::read_to_string(&file)?, &user)
            );
            Ok(())
        }
        Some("remove") => {
            if u2f_key_count(&content, &user) == 0 {
                println!("Keine Schlüssel gekoppelt.");
                return Ok(());
            }
            if !yes(&ask("Alle Sicherheitsschlüssel entfernen? [j/N] ")?) {
                println!("Abgebrochen.");
                return Ok(());
            }
            let rest: Vec<&str> = content
                .lines()
                .filter(|l| {
                    !l.strip_prefix(user.as_str())
                        .is_some_and(|r| r.starts_with(':'))
                })
                .filter(|l| !l.trim().is_empty())
                .collect();
            if rest.is_empty() {
                std::fs::remove_file(&file)?;
            } else {
                write_private(&file, &(rest.join("\n") + "\n"))?;
            }
            println!("Schlüssel entfernt.");
            Ok(())
        }
        Some("-h" | "--help" | "help") => {
            println!("{KEY_HELP}");
            Ok(())
        }
        Some(other) => bail!("key: unbekannt „{other}“ – siehe tuilnux key help"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(b: &[u8]) -> String {
        b.iter().map(|x| format!("{x:02x}")).collect()
    }

    #[test]
    fn sha1_and_hmac_vectors() {
        assert_eq!(
            hex(&sha1(b"abc")),
            "a9993e364706816aba3e25717850c26c9cd0d89d"
        );
        assert_eq!(hex(&sha1(b"")), "da39a3ee5e6b4b0d3255bfef95601890afd80709");
        let long = vec![b'a'; 1000];
        assert_eq!(
            hex(&sha1(&long)),
            "291e9a6c66994949b57ba5e650361e98fc36b1ba"
        );
        assert_eq!(
            hex(&hmac_sha1(
                b"key",
                b"The quick brown fox jumps over the lazy dog"
            )),
            "de7c9b85b8b78aa6bc8a7a36f70a90701c9db4d9"
        );
    }

    #[test]
    fn totp_rfc6238() {
        let secret = b"12345678901234567890";
        assert_eq!(hotp(secret, 59 / 30, 8), 94_287_082);
        assert_eq!(hotp(secret, 1_111_111_109 / 30, 8), 7_081_804);
        assert_eq!(hotp(secret, 0, 6), 755_224);
        assert!(totp_matches(secret, 59, "287082"));
        assert!(totp_matches(secret, 59 + 30, "287 082"));
        assert!(!totp_matches(secret, 59 + 90, "287082"));
        assert!(!totp_matches(secret, 59, "abc"));
    }

    #[test]
    fn base32_and_uri() {
        assert_eq!(base32(b"foobar"), "MZXW6YTBOI");
        assert_eq!(
            base32(b"12345678901234567890"),
            "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ"
        );
        let uri = otpauth_uri("ABC", "jonas", "arch");
        assert_eq!(
            uri,
            "otpauth://totp/TUILNUX%3Ajonas%40arch?secret=ABC&issuer=TUILNUX&algorithm=SHA1&digits=6&period=30"
        );
        let qr = qr_lines(&uri).unwrap();
        assert!(qr.len() > 10);
        assert!(qr[0].contains('█'));
    }

    #[test]
    fn google_authenticator_file() {
        let f = ga_file("ABC", &[12_345_678, 87_654_321]);
        assert_eq!(
            f,
            "ABC\n\" RATE_LIMIT 3 30\n\" WINDOW_SIZE 3\n\" DISALLOW_REUSE\n\" TOTP_AUTH\n12345678\n87654321\n"
        );
    }

    #[test]
    fn merges_u2f_lines() {
        let first = merge_u2f("", "user", "user:k1,p1,es256,+presence\n");
        assert_eq!(first, "user:k1,p1,es256,+presence\n");
        let second = merge_u2f(&first, "user", "user:k2,p2,es256,+presence");
        assert_eq!(second, "user:k1,p1,es256,+presence:k2,p2,es256,+presence\n");
        assert_eq!(u2f_key_count(&second, "user"), 2);
        let other = merge_u2f(&second, "anna", ":k3,p3,es256,+presence");
        assert!(other.ends_with("anna:k3,p3,es256,+presence\n"));
    }
}
