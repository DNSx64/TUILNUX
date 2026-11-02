//! Tastenkombination für den Manager-Modus der TUILNUX-Sitzung.

use anyhow::{Result, bail};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// Tastenkombination für den Manager-Modus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Prefix {
    pub code: KeyCode,
    pub modifiers: KeyModifiers,
}

impl Prefix {
    pub fn parse(s: &str) -> Result<Self> {
        let mut modifiers = KeyModifiers::NONE;
        let mut code = None;
        for part in s.split('+').map(|p| p.trim().to_lowercase()) {
            match part.as_str() {
                "ctrl" | "control" | "strg" => modifiers |= KeyModifiers::CONTROL,
                "alt" | "meta" => modifiers |= KeyModifiers::ALT,
                "shift" => modifiers |= KeyModifiers::SHIFT,
                "space" => code = Some(KeyCode::Char(' ')),
                "tab" => code = Some(KeyCode::Tab),
                "esc" | "escape" => code = Some(KeyCode::Esc),
                p if p.chars().count() == 1 => code = p.chars().next().map(KeyCode::Char),
                p if p.starts_with('f') && p[1..].parse::<u8>().is_ok() => {
                    code = Some(KeyCode::F(p[1..].parse()?))
                }
                p => bail!("unbekannte Taste „{p}“ in prefix = \"{s}\""),
            }
        }
        let Some(code) = code else {
            bail!("prefix = \"{s}\" enthält keine Taste");
        };
        if modifiers.is_empty() && !matches!(code, KeyCode::F(_)) {
            bail!(
                "prefix = \"{s}\" braucht Strg oder Alt (sonst ist die Taste nicht mehr tippbar)"
            );
        }
        Ok(Self { code, modifiers })
    }

    pub fn matches(&self, key: &KeyEvent) -> bool {
        let mods = key.modifiers - KeyModifiers::SHIFT;
        let want = self.modifiers - KeyModifiers::SHIFT;
        let same_code = match (self.code, key.code) {
            (KeyCode::Char(a), KeyCode::Char(b)) => a.eq_ignore_ascii_case(&b),
            (a, b) => a == b,
        };
        if same_code && mods == want {
            return true;
        }
        // Strg+Leertaste kommt je nach Terminal als NUL bzw. Strg+@ an.
        self.code == KeyCode::Char(' ')
            && want == KeyModifiers::CONTROL
            && (key.code == KeyCode::Null
                || (key.code == KeyCode::Char('@') && mods == KeyModifiers::CONTROL)
                || (key.code == KeyCode::Char('2') && mods == KeyModifiers::CONTROL))
    }

    pub fn label(&self) -> String {
        let mut out = String::new();
        if self.modifiers.contains(KeyModifiers::CONTROL) {
            out.push_str("Strg+");
        }
        if self.modifiers.contains(KeyModifiers::ALT) {
            out.push_str("Alt+");
        }
        if self.modifiers.contains(KeyModifiers::SHIFT) {
            out.push_str("Shift+");
        }
        match self.code {
            KeyCode::Char(' ') => out.push_str("Leertaste"),
            KeyCode::Char(c) => out.push(c.to_ascii_uppercase()),
            KeyCode::F(n) => out.push_str(&format!("F{n}")),
            KeyCode::Tab => out.push_str("Tab"),
            KeyCode::Esc => out.push_str("Esc"),
            other => out.push_str(&format!("{other:?}")),
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, modifiers)
    }

    #[test]
    fn default_prefix_is_ctrl_space() {
        let p = Prefix::parse("ctrl+space").unwrap();
        assert!(p.matches(&key(KeyCode::Char(' '), KeyModifiers::CONTROL)));
        assert!(p.matches(&key(KeyCode::Null, KeyModifiers::NONE)));
        assert!(p.matches(&key(KeyCode::Char('@'), KeyModifiers::CONTROL)));
        assert!(!p.matches(&key(KeyCode::Char(' '), KeyModifiers::NONE)));
        assert_eq!(p.label(), "Strg+Leertaste");
    }

    #[test]
    fn other_prefixes() {
        let p = Prefix::parse("Ctrl+A").unwrap();
        assert!(p.matches(&key(KeyCode::Char('a'), KeyModifiers::CONTROL)));
        assert!(!p.matches(&key(KeyCode::Char('a'), KeyModifiers::NONE)));
        assert_eq!(p.label(), "Strg+A");
        let p = Prefix::parse("alt+space").unwrap();
        assert!(p.matches(&key(KeyCode::Char(' '), KeyModifiers::ALT)));
        assert!(Prefix::parse("F12").is_ok());
        assert!(Prefix::parse("a").is_err());
        assert!(Prefix::parse("ctrl+").is_err());
        assert!(Prefix::parse("ctrl+hyper").is_err());
    }
}
