//! Sprachpakete für TUILNUX.
//!
//! Der Quelltext ist Deutsch. `tr("Benutzername")` liefert die Übersetzung der
//! aktiven Sprache oder – falls es keine gibt – den deutschen Text zurück.
//!
//! Ein Sprachpaket ist eine TOML-Datei:
//!
//! ```toml
//! [meta]
//! name = "English"
//!
//! [strings]
//! "Benutzername" = "User name"
//! ```
//!
//! Eingebaut sind `en`, `fr` und `es`. Weitere Pakete (oder Ergänzungen) werden
//! aus `/usr/share/tuilnux/lang`, `/etc/tuilnux/lang` und
//! `~/.config/tuilnux/lang` gelesen; der Dateiname ist der Sprachcode.
//! Was einem Paket fehlt, kommt aus dem englischen Paket.

use serde::Deserialize;
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::RwLock;

/// Sprache des Quelltexts.
pub const SOURCE: &str = "de";

const BUILTIN: &[(&str, &str)] = &[
    ("en", include_str!("../lang/en.toml")),
    ("fr", include_str!("../lang/fr.toml")),
    ("es", include_str!("../lang/es.toml")),
];

#[derive(Debug, Default, Deserialize)]
struct PackFile {
    #[serde(default)]
    meta: Meta,
    #[serde(default)]
    strings: HashMap<String, String>,
}

#[derive(Debug, Default, Deserialize)]
struct Meta {
    #[serde(default)]
    name: String,
}

struct Catalog {
    code: String,
    strings: HashMap<String, String>,
}

static ACTIVE: RwLock<Option<&'static Catalog>> = RwLock::new(None);

/// Ordner, in denen zusätzliche Sprachpakete liegen dürfen (in Ladereihenfolge).
pub fn pack_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Ok(d) = std::env::var("TUILNUX_LANG_DIR") {
        dirs.push(PathBuf::from(d));
    }
    dirs.push(PathBuf::from("/usr/share/tuilnux/lang"));
    dirs.push(PathBuf::from("/etc/tuilnux/lang"));
    if let Some(c) = dirs::config_dir() {
        dirs.push(c.join("tuilnux/lang"));
    }
    dirs
}

fn parse(text: &str) -> Option<PackFile> {
    toml::from_str(text).ok()
}

/// Alle verfügbaren Sprachen als `(Code, Name)`, Deutsch zuerst.
pub fn available() -> Vec<(String, String)> {
    let mut found: BTreeMap<String, String> = BTreeMap::new();
    for (code, text) in BUILTIN {
        let name = parse(text).map(|p| p.meta.name).unwrap_or_default();
        found.insert((*code).into(), name);
    }
    for dir in pack_dirs() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in entries.flatten() {
            let path = e.path();
            if path.extension().and_then(|x| x.to_str()) != Some("toml") {
                continue;
            }
            let Some(code) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            let code = code.to_ascii_lowercase();
            if code == SOURCE || !valid_code(&code) {
                continue;
            }
            let name = std::fs::read_to_string(&path)
                .ok()
                .and_then(|t| parse(&t))
                .map(|p| p.meta.name)
                .unwrap_or_default();
            let entry = found.entry(code).or_default();
            if entry.is_empty() {
                *entry = name;
            }
        }
    }
    let mut out = vec![(SOURCE.to_string(), "Deutsch".to_string())];
    out.extend(found.into_iter().map(|(c, n)| {
        let n = if n.is_empty() { c.clone() } else { n };
        (c, n)
    }));
    out
}

fn valid_code(code: &str) -> bool {
    (2..=3).contains(&code.len()) && code.chars().all(|c| c.is_ascii_lowercase())
}

/// Sprachcode aus einem Locale-Wert wie `de_DE.UTF-8` oder `en`.
pub fn locale_code(value: &str) -> Option<String> {
    let v = value.trim().trim_matches('"');
    let code: String = v
        .chars()
        .take_while(|c| !matches!(c, '_' | '.' | '@' | '-' | ':'))
        .collect::<String>()
        .to_ascii_lowercase();
    if code == "c" || code == "posix" || !valid_code(&code) {
        return None;
    }
    Some(code)
}

/// Liest `LANG=` aus dem Inhalt einer `locale.conf`.
pub fn locale_conf_lang(text: &str) -> Option<String> {
    text.lines()
        .filter_map(|l| l.trim().strip_prefix("LANG="))
        .find_map(locale_code)
}

/// Bestimmt den Sprachcode aus einer Einstellung (`auto`, `de`, `en` …).
/// `auto` nutzt `TUILNUX_LANG`, `LC_ALL`, `LC_MESSAGES`, `LANG` und zuletzt
/// `/etc/locale.conf`. Unbekannte Sprachen fallen auf Englisch zurück.
pub fn resolve(setting: &str) -> String {
    let s = setting.trim().to_ascii_lowercase();
    let wanted = if !s.is_empty() && s != "auto" {
        locale_code(&s)
    } else {
        ["TUILNUX_LANG", "LC_ALL", "LC_MESSAGES", "LANG"]
            .iter()
            .filter_map(|v| std::env::var(v).ok())
            .find_map(|v| locale_code(&v))
            .or_else(|| {
                std::fs::read_to_string("/etc/locale.conf")
                    .ok()
                    .and_then(|t| locale_conf_lang(&t))
            })
    };
    let Some(code) = wanted else {
        return SOURCE.into();
    };
    if available().iter().any(|(c, _)| *c == code) {
        code
    } else {
        "en".into()
    }
}

fn load_layer(code: &str, strings: &mut HashMap<String, String>) {
    if let Some((_, text)) = BUILTIN.iter().find(|(c, _)| *c == code)
        && let Some(p) = parse(text)
    {
        strings.extend(p.strings);
    }
    for dir in pack_dirs() {
        if let Ok(text) = std::fs::read_to_string(dir.join(format!("{code}.toml")))
            && let Some(p) = parse(&text)
        {
            strings.extend(p.strings.into_iter().filter(|(_, v)| !v.is_empty()));
        }
    }
}

/// Fehlende Einträge eines Pakets kommen aus dem englischen Paket.
fn build(code: &str) -> Catalog {
    let mut strings = HashMap::new();
    if code != SOURCE {
        if code != "en" {
            load_layer("en", &mut strings);
        }
        load_layer(code, &mut strings);
    }
    Catalog {
        code: code.into(),
        strings,
    }
}

/// Aktiviert die Sprache aus der Einstellung und gibt den Code zurück.
pub fn init(setting: &str) -> String {
    let code = resolve(setting);
    let same = ACTIVE
        .read()
        .map(|g| g.is_some_and(|c| c.code == code))
        .unwrap_or(false);
    if !same {
        let cat: &'static Catalog = Box::leak(Box::new(build(&code)));
        *ACTIVE.write().unwrap_or_else(|e| e.into_inner()) = Some(cat);
    }
    code
}

/// Code der aktiven Sprache (`de`, wenn noch nichts geladen wurde).
pub fn current() -> String {
    ACTIVE
        .read()
        .ok()
        .and_then(|g| g.map(|c| c.code.clone()))
        .unwrap_or_else(|| SOURCE.into())
}

/// Übersetzt einen deutschen Text in die aktive Sprache.
pub fn tr(s: &str) -> &str {
    let guard = ACTIVE.read().unwrap_or_else(|e| e.into_inner());
    if let Some(cat) = *guard
        && let Some(t) = cat.strings.get(s)
    {
        return t.as_str();
    }
    s
}

/// Übersetzt und ersetzt der Reihe nach jedes `{}` durch ein Argument.
pub fn trf(s: &str, args: &[&dyn std::fmt::Display]) -> String {
    let mut out = String::new();
    let mut rest = tr(s);
    let mut it = args.iter();
    while let Some(pos) = rest.find("{}") {
        out.push_str(&rest[..pos]);
        match it.next() {
            Some(a) => out.push_str(&a.to_string()),
            None => out.push_str("{}"),
        }
        rest = &rest[pos + 2..];
    }
    out.push_str(rest);
    out
}

/// Locale für `localectl`, z. B. `de` → `de_DE.UTF-8`, `en` → `en_US.UTF-8`.
pub fn locale_for(code: &str) -> String {
    let region = match code {
        "en" => "US".to_string(),
        "ja" => "JP".to_string(),
        "uk" => "UA".to_string(),
        "cs" => "CZ".to_string(),
        "da" => "DK".to_string(),
        "sv" => "SE".to_string(),
        other => other.to_ascii_uppercase(),
    };
    format!("{code}_{region}.UTF-8")
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locales_for_codes() {
        assert_eq!(locale_for("de"), "de_DE.UTF-8");
        assert_eq!(locale_for("en"), "en_US.UTF-8");
        assert_eq!(locale_for("fr"), "fr_FR.UTF-8");
    }

    #[test]
    fn builtin_packs_parse_and_keep_placeholders() {
        for (code, text) in BUILTIN {
            let p: PackFile = toml::from_str(text).unwrap_or_else(|e| panic!("{code}: {e}"));
            assert!(!p.meta.name.is_empty(), "{code} ohne Namen");
            assert!(!p.strings.is_empty(), "{code} leer");
            for (k, v) in &p.strings {
                assert_eq!(
                    k.matches("{}").count(),
                    v.matches("{}").count(),
                    "{code}: Platzhalter in {k:?}"
                );
            }
        }
    }

    #[test]
    fn english_covers_every_other_pack() {
        let en: PackFile = toml::from_str(BUILTIN[0].1).unwrap();
        for (code, text) in &BUILTIN[1..] {
            let p: PackFile = toml::from_str(text).unwrap();
            for k in p.strings.keys() {
                assert!(en.strings.contains_key(k), "{code}: {k:?} fehlt in en");
            }
        }
    }

    #[test]
    fn locale_codes() {
        assert_eq!(locale_code("de_DE.UTF-8").as_deref(), Some("de"));
        assert_eq!(locale_code("en").as_deref(), Some("en"));
        assert_eq!(locale_code("C.UTF-8"), None);
        assert_eq!(locale_code("POSIX"), None);
        assert_eq!(
            locale_conf_lang("# x\nLANG=fr_FR.UTF-8\n").as_deref(),
            Some("fr")
        );
    }

    #[test]
    fn translate_and_format() {
        init("en");
        assert_eq!(current(), "en");
        assert_eq!(tr("Benutzername"), "User name");
        assert_eq!(tr("kein Schlüssel im Paket"), "kein Schlüssel im Paket");
        assert_eq!(trf("Schritt {} von {}", &[&2, &7]), "Step 2 of 7");
        init("de");
        assert_eq!(tr("Benutzername"), "Benutzername");
        assert_eq!(trf("Schritt {} von {}", &[&2, &7]), "Schritt 2 von 7");
    }
}
