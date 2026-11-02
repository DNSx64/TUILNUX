//! Installierte Programme für die Programmsuche (Super+F): `.desktop`-Dateien
//! nach XDG (auch Flatpak), die TUILNUX-Werkzeuge und Befehle im `PATH`.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use tuilnux_ui::tr;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// Mitgelieferte TUILNUX-Werkzeuge (tfiles, tedit …).
    Builtin,
    /// Eintrag aus einer `.desktop`-Datei.
    Desktop,
    /// Ausführbare Datei im `PATH` ohne `.desktop`-Datei.
    Path,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AppEntry {
    /// Eindeutig für den Verlauf: Desktop-Datei-ID, `builtin:…` oder `path:…`.
    pub id: String,
    pub name: String,
    pub generic: String,
    pub comment: String,
    pub keywords: Vec<String>,
    pub exec: Vec<String>,
    /// Braucht ein Terminal (öffnet sich als TUILNUX-Fenster).
    pub terminal: bool,
    pub source: Source,
}

impl AppEntry {
    /// Name der ausführbaren Datei ohne Pfad.
    pub fn program(&self) -> &str {
        self.exec
            .first()
            .map_or("", |p| p.rsplit('/').next().unwrap_or(p))
    }

    /// Kurzbeschreibung für die Trefferliste.
    pub fn summary(&self) -> &str {
        if self.comment.is_empty() {
            &self.generic
        } else {
            &self.comment
        }
    }
}

/// Sprachschlüssel für `Name[…]` in der Reihenfolge der Desktop-Entry-Spezifikation:
/// `de_DE.UTF-8@euro` → `de_DE@euro`, `de_DE`, `de@euro`, `de`.
pub fn locales(language: &str) -> Vec<String> {
    let raw = if language.is_empty() || language == "auto" {
        ["LC_ALL", "LC_MESSAGES", "LANG"]
            .iter()
            .find_map(|k| std::env::var(k).ok().filter(|v| !v.is_empty()))
            .unwrap_or_default()
    } else {
        language.to_string()
    };
    let (rest, modifier) = match raw.split_once('@') {
        Some((r, m)) => (r, Some(m)),
        None => (raw.as_str(), None),
    };
    let rest = rest.split('.').next().unwrap_or_default();
    let (lang, country) = match rest.split_once('_') {
        Some((l, c)) => (l, Some(c)),
        None => (rest, None),
    };
    if lang.is_empty() || lang == "C" || lang == "POSIX" {
        return Vec::new();
    }
    let mut out = Vec::new();
    if let (Some(c), Some(m)) = (country, modifier) {
        out.push(format!("{lang}_{c}@{m}"));
    }
    if let Some(c) = country {
        out.push(format!("{lang}_{c}"));
    }
    if let Some(m) = modifier {
        out.push(format!("{lang}@{m}"));
    }
    out.push(lang.to_string());
    out
}

/// Löst die Escapes in Werten auf (`\s`, `\n`, `\t`, `\r`, `\\`).
fn unescape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('s') => out.push(' '),
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some(other) => out.push(other),
            None => out.push('\\'),
        }
    }
    out
}

/// Zerlegt eine `Exec`-Zeile in Argumente (Anführungszeichen nach der Spezifikation)
/// und ersetzt die Feldcodes: Datei-/URL-Platzhalter entfallen, `%c` wird der
/// Name, `%k` der Pfad der Datei, `%%` ein Prozentzeichen.
pub fn split_exec(exec: &str, name: &str, file: &str) -> Vec<String> {
    let mut args = Vec::new();
    let (mut cur, mut quoted, mut started) = (String::new(), false, false);
    let mut chars = exec.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                quoted = !quoted;
                started = true;
            }
            '\\' if quoted => {
                if let Some(n) = chars.next() {
                    cur.push(n);
                }
            }
            c if c.is_whitespace() && !quoted => {
                if started || !cur.is_empty() {
                    args.push(std::mem::take(&mut cur));
                    started = false;
                }
            }
            c => cur.push(c),
        }
    }
    if started || !cur.is_empty() {
        args.push(cur);
    }
    let mut out = Vec::with_capacity(args.len());
    for arg in args {
        if matches!(
            arg.as_str(),
            "%f" | "%F" | "%u" | "%U" | "%d" | "%D" | "%n" | "%N" | "%v" | "%m" | "%i"
        ) {
            continue;
        }
        let mut s = String::with_capacity(arg.len());
        let mut it = arg.chars();
        while let Some(c) = it.next() {
            if c != '%' {
                s.push(c);
                continue;
            }
            match it.next() {
                Some('%') => s.push('%'),
                Some('c') => s.push_str(name),
                Some('k') => s.push_str(file),
                _ => {}
            }
        }
        if !s.is_empty() || arg.is_empty() {
            out.push(s);
        }
    }
    out
}

fn in_path(program: &str) -> bool {
    if program.contains('/') {
        return Path::new(program).is_file();
    }
    std::env::var_os("PATH")
        .is_some_and(|paths| std::env::split_paths(&paths).any(|dir| dir.join(program).is_file()))
}

/// Liest eine `.desktop`-Datei. `None` für versteckte Einträge, Links,
/// Verzeichnisse und Programme, deren `TryExec` fehlt.
pub fn parse_desktop(content: &str, id: &str, file: &str, locales: &[String]) -> Option<AppEntry> {
    let mut fields: HashMap<&str, &str> = HashMap::new();
    let mut in_entry = false;
    for line in content.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_entry = line == "[Desktop Entry]";
            continue;
        }
        if !in_entry || line.starts_with('#') {
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            fields.entry(key.trim()).or_insert(value.trim());
        }
    }
    let get = |key: &str| -> Option<String> {
        locales
            .iter()
            .find_map(|l| fields.get(format!("{key}[{l}]").as_str()))
            .or_else(|| fields.get(key))
            .map(|v| unescape(v))
    };
    let yes = |key: &str| fields.get(key).is_some_and(|v| *v == "true");
    if fields.get("Type").copied() != Some("Application") || yes("NoDisplay") || yes("Hidden") {
        return None;
    }
    if let Some(try_exec) = get("TryExec")
        && !try_exec.is_empty()
        && !in_path(&try_exec)
    {
        return None;
    }
    let name = get("Name").filter(|n| !n.is_empty())?;
    let exec = split_exec(&get("Exec")?, &name, file);
    if exec.is_empty() {
        return None;
    }
    let keywords = get("Keywords")
        .map(|k| {
            k.split(';')
                .map(str::trim)
                .filter(|w| !w.is_empty())
                .map(String::from)
                .collect()
        })
        .unwrap_or_default();
    Some(AppEntry {
        id: id.to_string(),
        name,
        generic: get("GenericName").unwrap_or_default(),
        comment: get("Comment").unwrap_or_default(),
        keywords,
        exec,
        terminal: yes("Terminal"),
        source: Source::Desktop,
    })
}

/// Die mitgelieferten Werkzeuge; `shell` startet ein neues Terminal-Fenster.
pub fn builtins(shell: Vec<String>) -> Vec<AppEntry> {
    let tool = |name: &str| tuilnux_ui::tool_path(name).display().to_string();
    let entry = |id: &str, name: &str, comment: &str, keywords: &str, exec: Vec<String>| AppEntry {
        id: format!("builtin:{id}"),
        name: tr(name).to_string(),
        generic: String::new(),
        comment: tr(comment).to_string(),
        keywords: keywords.split(' ').map(String::from).collect(),
        exec,
        terminal: true,
        source: Source::Builtin,
    };
    vec![
        entry(
            "terminal",
            "Terminal",
            "Neues Terminal-Fenster mit der Shell",
            "shell konsole console bash",
            shell,
        ),
        entry(
            "tfiles",
            "Dateien",
            "Dateimanager (tfiles)",
            "files explorer dateimanager ordner",
            vec![tool("tfiles")],
        ),
        entry(
            "tedit",
            "Editor",
            "Texteditor (tedit)",
            "text editor notepad",
            vec![tool("tedit")],
        ),
        entry(
            "ttop",
            "Systemmonitor",
            "Prozesse, CPU und Arbeitsspeicher (ttop)",
            "taskmanager prozesse processes top htop",
            vec![tool("ttop")],
        ),
        entry(
            "tsettings",
            "Einstellungen",
            "TUILNUX einrichten (tsettings)",
            "settings config optionen",
            vec![tool("tsettings")],
        ),
    ]
}

/// Desktop-Datei-ID: Pfad unterhalb von `applications`, `/` wird zu `-`.
fn desktop_id(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('/', "-")
}

fn scan_dir(
    root: &Path,
    dir: &Path,
    depth: u8,
    locales: &[String],
    seen: &mut HashSet<String>,
    out: &mut Vec<AppEntry>,
) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut paths: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
    paths.sort();
    for path in paths {
        if path.is_dir() {
            if depth < 4 {
                scan_dir(root, &path, depth + 1, locales, seen, out);
            }
            continue;
        }
        if path.extension().is_none_or(|e| e != "desktop") {
            continue;
        }
        let id = desktop_id(root, &path);
        // Die erste Fundstelle gewinnt, auch wenn sie den Eintrag versteckt.
        if !seen.insert(id.clone()) {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };
        if let Some(entry) = parse_desktop(&content, &id, &path.to_string_lossy(), locales) {
            out.push(entry);
        }
    }
}

#[cfg(unix)]
fn executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn executable(path: &Path) -> bool {
    path.is_file()
}

/// Alle Programme: Werkzeuge, `.desktop`-Einträge, dann übrige Befehle im `PATH`.
pub fn installed(locales: &[String], shell: Vec<String>) -> Vec<AppEntry> {
    let mut out = builtins(shell);
    let mut seen = HashSet::new();
    for dir in tuilnux_config::application_dirs() {
        scan_dir(&dir, &dir, 0, locales, &mut seen, &mut out);
    }
    let mut known: HashSet<String> = out.iter().map(|e| e.program().to_string()).collect();
    if let Some(paths) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&paths) {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            let mut names: Vec<String> = entries
                .flatten()
                .filter_map(|e| e.file_name().into_string().ok())
                .filter(|n| !n.starts_with('.'))
                .collect();
            names.sort();
            for name in names {
                if known.contains(&name) || !executable(&dir.join(&name)) {
                    continue;
                }
                known.insert(name.clone());
                out.push(AppEntry {
                    id: format!("path:{name}"),
                    name: name.clone(),
                    generic: String::new(),
                    comment: String::new(),
                    keywords: Vec::new(),
                    exec: vec![name],
                    terminal: true,
                    source: Source::Path,
                });
            }
        }
    }
    out
}

/// Kommen die Zeichen von `needle` der Reihe nach in `hay` vor?
fn subsequence(hay: &str, needle: &str) -> bool {
    let mut it = hay.chars();
    needle.chars().all(|n| it.any(|h| h == n))
}

fn word_starts(text: &str, word: &str) -> bool {
    text.split(|c: char| !c.is_alphanumeric())
        .any(|p| p.starts_with(word))
}

fn word_score(e: &AppEntry, w: &str) -> u32 {
    let name = e.name.to_lowercase();
    let program = e.program().to_lowercase();
    let mut best = if name == w {
        1000
    } else if name.starts_with(w) {
        800
    } else if word_starts(&name, w) {
        600
    } else if name.contains(w) {
        400
    } else {
        0
    };
    best = best.max(if program == w {
        700
    } else if program.starts_with(w) {
        500
    } else if program.contains(w) {
        250
    } else {
        0
    });
    let generic = e.generic.to_lowercase();
    let keyword = |f: &dyn Fn(&str) -> bool| e.keywords.iter().any(|k| f(&k.to_lowercase()));
    if word_starts(&generic, w) || keyword(&|k| k.starts_with(w)) {
        best = best.max(300);
    } else if generic.contains(w) || keyword(&|k| k.contains(w)) {
        best = best.max(200);
    }
    if best == 0 && e.comment.to_lowercase().contains(w) {
        best = 100;
    }
    // Lose Treffer (f…i…r…e) nur bei echten Programmen, nicht bei tausenden Befehlen.
    if best == 0 && e.source != Source::Path && subsequence(&name, w) {
        best = 50;
    }
    best
}

/// Punkte für `query` (0 = passt nicht). Alle Wörter müssen irgendwo vorkommen.
pub fn score(e: &AppEntry, query: &str) -> u32 {
    let query = query.to_lowercase();
    let mut total = 0;
    for w in query.split_whitespace() {
        match word_score(e, w) {
            0 => return 0,
            s => total += s,
        }
    }
    total
}

/// Indizes der Treffer, beste zuerst. Ohne Suchtext: alle Programme außer den
/// reinen `PATH`-Befehlen, häufig gestartete zuerst.
pub fn rank(apps: &[AppEntry], query: &str, history: &HashMap<String, u32>) -> Vec<usize> {
    let empty = query.trim().is_empty();
    let mut hits: Vec<(u32, usize)> = apps
        .iter()
        .enumerate()
        .filter_map(|(i, e)| {
            let uses = history.get(&e.id).copied().unwrap_or(0).min(50);
            if empty {
                return (e.source != Source::Path).then_some((uses * 20, i));
            }
            let mut s = score(e, query);
            if s == 0 {
                return None;
            }
            if e.source == Source::Path {
                s /= 3;
            }
            Some((s + uses * 20, i))
        })
        .collect();
    hits.sort_by(|a, b| {
        b.0.cmp(&a.0).then_with(|| {
            let (x, y) = (&apps[a.1], &apps[b.1]);
            let shorter = if empty {
                std::cmp::Ordering::Equal
            } else {
                x.name.chars().count().cmp(&y.name.chars().count())
            };
            (x.source == Source::Path)
                .cmp(&(y.source == Source::Path))
                .then(shorter)
                .then_with(|| x.name.to_lowercase().cmp(&y.name.to_lowercase()))
        })
    });
    hits.into_iter().map(|(_, i)| i).collect()
}

fn history_path() -> Option<PathBuf> {
    let state = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/state")))?;
    Some(state.join("tuilnux/search-history"))
}

/// Wie oft welches Programm über die Suche gestartet wurde.
pub fn load_history() -> HashMap<String, u32> {
    history_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .map(|s| parse_history(&s))
        .unwrap_or_default()
}

fn parse_history(text: &str) -> HashMap<String, u32> {
    text.lines()
        .filter_map(|l| {
            let (n, id) = l.split_once(' ')?;
            Some((id.to_string(), n.parse().ok()?))
        })
        .collect()
}

fn format_history(history: &HashMap<String, u32>) -> String {
    let mut items: Vec<_> = history.iter().collect();
    items.sort_by(|a, b| b.1.cmp(a.1).then_with(|| a.0.cmp(b.0)));
    items
        .into_iter()
        .take(200)
        .map(|(id, n)| format!("{n} {id}\n"))
        .collect()
}

pub fn record_use(id: &str) {
    let Some(path) = history_path() else {
        return;
    };
    let mut history = load_history();
    *history.entry(id.to_string()).or_insert(0) += 1;
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(path, format_history(&history));
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIREFOX: &str = "\
[Desktop Entry]
Version=1.0
Name=Firefox Web Browser
Name[de]=Firefox-Webbrowser
GenericName=Web Browser
GenericName[de]=Webbrowser
Comment=Browse the World Wide Web
Comment[de]=Im Internet surfen
Keywords=Internet;WWW;Browser;Web;Explorer
Exec=/usr/lib/firefox/firefox %u
Terminal=false
Type=Application

[Desktop Action new-window]
Name=New Window
Exec=/usr/lib/firefox/firefox --new-window %u
";

    fn de() -> Vec<String> {
        locales("de_DE.UTF-8")
    }

    #[test]
    fn locale_fallbacks() {
        assert_eq!(locales("de_DE.UTF-8"), ["de_DE", "de"]);
        assert_eq!(
            locales("sr_RS@latin"),
            ["sr_RS@latin", "sr_RS", "sr@latin", "sr"]
        );
        assert_eq!(locales("en"), ["en"]);
        assert!(locales("C").is_empty());
    }

    #[test]
    fn parses_desktop_files() {
        let e = parse_desktop(FIREFOX, "firefox.desktop", "/x/firefox.desktop", &de()).unwrap();
        assert_eq!(e.name, "Firefox-Webbrowser");
        assert_eq!(e.generic, "Webbrowser");
        assert_eq!(e.summary(), "Im Internet surfen");
        assert_eq!(e.exec, ["/usr/lib/firefox/firefox"]);
        assert_eq!(e.program(), "firefox");
        assert!(!e.terminal);
        assert_eq!(e.keywords.len(), 5);
        let en = parse_desktop(FIREFOX, "f", "f", &locales("en_US")).unwrap();
        assert_eq!(en.name, "Firefox Web Browser");

        let hidden = "[Desktop Entry]\nType=Application\nName=X\nExec=x\nNoDisplay=true\n";
        assert!(parse_desktop(hidden, "x", "x", &[]).is_none());
        let link = "[Desktop Entry]\nType=Link\nName=X\nURL=https://x\n";
        assert!(parse_desktop(link, "x", "x", &[]).is_none());
        let missing = "[Desktop Entry]\nType=Application\nName=X\nExec=x\nTryExec=/nicht/da\n";
        assert!(parse_desktop(missing, "x", "x", &[]).is_none());
        let htop = "[Desktop Entry]\nType=Application\nName=Htop\nExec=htop\nTerminal=true\n";
        assert!(
            parse_desktop(htop, "htop.desktop", "x", &[])
                .unwrap()
                .terminal
        );
    }

    #[test]
    fn exec_quoting_and_field_codes() {
        assert_eq!(
            split_exec("vlc --started-from-file %U", "VLC", "f"),
            ["vlc", "--started-from-file"]
        );
        assert_eq!(
            split_exec(
                r#""/opt/My App/run" --name=%c --file "%k" 100%%"#,
                "App",
                "/a.desktop"
            ),
            [
                "/opt/My App/run",
                "--name=App",
                "--file",
                "/a.desktop",
                "100%"
            ]
        );
        assert_eq!(
            split_exec(r#"sh -c "echo \"hi\" \$HOME""#, "x", "f"),
            ["sh", "-c", r#"echo "hi" $HOME"#]
        );
        assert_eq!(split_exec(r#"prog "" x"#, "x", "f"), ["prog", "", "x"]);
        // Erst die Escapes der Datei, dann die Anführungszeichen von Exec.
        let content =
            "[Desktop Entry]\nType=Application\nName=E\nExec=sh -c \"echo \\\\\"a b\\\\\"\"\n";
        let e = parse_desktop(content, "e", "e", &[]).unwrap();
        assert_eq!(e.exec, ["sh", "-c", "echo \"a b\""]);
    }

    fn app(name: &str, program: &str, source: Source) -> AppEntry {
        AppEntry {
            id: format!("{name}.desktop"),
            name: name.into(),
            generic: String::new(),
            comment: String::new(),
            keywords: Vec::new(),
            exec: vec![program.into()],
            terminal: false,
            source,
        }
    }

    #[test]
    fn ranking_prefers_name_matches_and_history() {
        let mut browser = app("Firefox", "firefox", Source::Desktop);
        browser.keywords = vec!["Internet".into(), "Web".into()];
        let apps = vec![
            browser,
            app("GNU Image Manipulation Program", "gimp", Source::Desktop),
            app("fire", "fire", Source::Path),
            app("copyfilerange", "copyfilerange", Source::Path),
            app("pycman-database", "pycman-database", Source::Path),
            app("python", "python", Source::Path),
            app("Files", "nautilus", Source::Desktop),
            app("Text Editor", "gnome-text-editor", Source::Desktop),
        ];
        let none = HashMap::new();
        let names = |q: &str, h: &HashMap<String, u32>| -> Vec<String> {
            rank(&apps, q, h)
                .into_iter()
                .map(|i| apps[i].name.clone())
                .collect()
        };
        assert_eq!(names("fire", &none), ["Firefox", "fire"]);
        assert_eq!(names("web", &none), ["Firefox"]);
        assert_eq!(names("gimp", &none), ["GNU Image Manipulation Program"]);
        assert_eq!(
            names("image prog", &none),
            ["GNU Image Manipulation Program"]
        );
        assert_eq!(names("edit", &none), ["Text Editor"]);
        assert_eq!(names("fi", &none)[..2], ["Files", "Firefox"]);
        assert!(names("zzz", &none).is_empty());
        assert_eq!(
            names("py", &none),
            ["python", "pycman-database", "copyfilerange"]
        );
        // Ohne Suchtext keine PATH-Befehle, häufig benutzte zuerst.
        let used: HashMap<String, u32> = [("Text Editor.desktop".to_string(), 3)].into();
        let all = names("", &used);
        assert_eq!(all.len(), 4);
        assert_eq!(all[0], "Text Editor");
        assert_eq!(
            names("fi", &[("Firefox.desktop".to_string(), 2)].into())[0],
            "Firefox"
        );
    }

    #[test]
    fn history_round_trip() {
        let h: HashMap<String, u32> =
            [("a.desktop".to_string(), 2), ("path:x y".to_string(), 5)].into();
        let text = format_history(&h);
        assert_eq!(text, "5 path:x y\n2 a.desktop\n");
        assert_eq!(parse_history(&text), h);
        assert!(parse_history("kaputt\nzwei drei\n").is_empty());
    }
}
