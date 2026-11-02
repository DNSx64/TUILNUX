//! Einstellungen der Sitzung aus `~/.config/tuilnux/tuilnux.toml`
//! (Modell und Dateiformat: Crate `tuilnux-config`, bearbeitbar mit `tsettings`).

use anyhow::Result;
pub use tuilnux_config::Settings as Config;
pub use tuilnux_ui::Prefix;

/// Lädt und prüft die Einstellungen; eine fehlende Datei ergibt die Standardwerte.
pub fn load() -> Result<Config> {
    let config = Config::load()?;
    check(&config)?;
    Ok(config)
}

/// Prüfung, die über `tuilnux-config` hinausgeht (Tastenkombination).
pub fn check(config: &Config) -> Result<()> {
    Prefix::parse(&config.prefix)?;
    Ok(())
}

/// Teilt eine Befehlszeile an Leerzeichen; "…" und '…' halten Teile zusammen.
pub fn split_command(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quote = None;
    let mut has = false;
    for c in s.chars() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), c) => cur.push(c),
            (None, '"' | '\'') => {
                quote = Some(c);
                has = true;
            }
            (None, c) if c.is_whitespace() => {
                if has || !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
                has = false;
            }
            (None, c) => cur.push(c),
        }
    }
    if has || !cur.is_empty() {
        out.push(cur);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(s: &str) -> Result<Config> {
        let c = Config::parse(s)?;
        check(&c)?;
        Ok(c)
    }

    #[test]
    fn parses_config_file() {
        let c = parse(
            "prefix = \"ctrl+b\"\nshell = [\"/bin/zsh\"]\nautostart = [\"ttop\", \"tedit 'a b.txt'\"]",
        )
        .unwrap();
        assert_eq!(c.shell, ["/bin/zsh"]);
        assert_eq!(split_command(&c.autostart[1]), ["tedit", "a b.txt"]);
        assert!(parse("prefix = \"x\"").is_err());
        assert!(parse("unbekannt = 1").is_err());
        assert!(parse("[bar]\nclock_format = \"%Q\"").is_err());
        assert_eq!(parse("").unwrap().prefix, "ctrl+space");
    }

    #[test]
    fn splits_commands() {
        assert_eq!(split_command("  ttop  "), ["ttop"]);
        assert_eq!(split_command("a \"\" b"), ["a", "", "b"]);
        assert!(split_command("").is_empty());
    }
}
