//! Shell-Integration: Prompt (PS1), Begrüßung, Verlauf, Aliase, Umgebung und
//! Hintergrundstart grafischer Programme. `tuilnux shell-init bash` gibt das
//! Skript aus, `/usr/share/tuilnux/bashrc.sh` lädt es per `eval`.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

use crate::{Choice, Palette, Rgb, Settings, parse_hex};

choice!(
    /// Aussehen der Eingabeaufforderung.
    PromptStyle [Tuilnux] {
        Tuilnux = "tuilnux", "TUILNUX (farbig, Git, Status)";
        TwoLine = "twoline", "zweizeilig";
        Classic = "classic", "klassisch [user@host ordner]$";
        Minimal = "minimal", "minimal (ordner ❯)";
        Custom = "custom", "eigenes PS1";
        System = "system", "nicht ändern (~/.bashrc)";
    }
);

choice!(
    /// Wie der aktuelle Ordner im Prompt erscheint.
    PathStyle [Full] {
        Full = "full", "voller Pfad (~/a/b/c)";
        Trimmed = "trimmed", "gekürzt (~/…/b/c)";
        Short = "short", "nur Ordnername (c)";
    }
);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct BashSettings {
    /// ASCIIA-Begrüßung beim Öffnen der Shell.
    pub welcome: bool,
    /// Eigene `.ASCIIA`-Datei (leer = ~/.config/tuilnux/welcome.ASCIIA bzw. mitgeliefert).
    pub welcome_file: String,
    /// Zusätzliche Zeile unter der Begrüßung.
    pub welcome_text: String,
    /// Begrüßung auch in neuen Terminal-Fenstern, nicht nur im Desktop.
    pub welcome_in_windows: bool,
    pub prompt: PromptStyle,
    /// PS1 für [`PromptStyle::Custom`], z. B. `\u@\h:\w\$ `.
    pub prompt_custom: String,
    pub path_style: PathStyle,
    pub prompt_user: bool,
    pub prompt_host: bool,
    pub prompt_git: bool,
    /// Rückgabewert des letzten Befehls anzeigen, wenn er nicht 0 war.
    pub prompt_status: bool,
    pub prompt_time: bool,
    /// Farben (#rrggbb, leer = Farbschema).
    pub prompt_user_color: Option<String>,
    pub prompt_path_color: Option<String>,
    pub prompt_symbol: String,
    /// Fenstertitel (Taskleiste) auf user@host: ordner setzen.
    pub terminal_title: bool,
    pub history_size: u32,
    pub history_ignore_dups: bool,
    /// Verlauf sofort zwischen allen offenen Shells teilen.
    pub history_shared: bool,
    pub completion: bool,
    /// `ls`, `grep` und `diff` farbig.
    pub color_aliases: bool,
    /// `NAME=BEFEHL`
    pub aliases: Vec<String>,
    /// `NAME=WERT` (`$HOME` usw. werden ersetzt).
    pub env: Vec<String>,
    /// Zusätzliche Ordner für `PATH`.
    pub path: Vec<String>,
    pub editor: String,
    /// Grafische Programme aus der Shell im Hintergrund starten (Shell bleibt frei).
    pub gui_background: bool,
    /// Weitere Programme, die im Hintergrund starten.
    pub gui_apps: Vec<String>,
    /// Programme, die nie im Hintergrund starten.
    pub gui_exclude: Vec<String>,
}

impl Default for BashSettings {
    fn default() -> Self {
        Self {
            welcome: true,
            welcome_file: String::new(),
            welcome_text: String::new(),
            welcome_in_windows: false,
            prompt: PromptStyle::Tuilnux,
            prompt_custom: r"\u@\h:\w\$ ".into(),
            path_style: PathStyle::Full,
            prompt_user: true,
            prompt_host: true,
            prompt_git: true,
            prompt_status: true,
            prompt_time: false,
            prompt_user_color: None,
            prompt_path_color: None,
            prompt_symbol: "❯".into(),
            terminal_title: true,
            history_size: 10_000,
            history_ignore_dups: true,
            history_shared: false,
            completion: true,
            color_aliases: true,
            aliases: vec!["ll=ls -lah".into(), "la=ls -A".into()],
            env: Vec::new(),
            path: vec!["~/.local/bin".into()],
            editor: "tedit".into(),
            gui_background: true,
            gui_apps: Vec::new(),
            gui_exclude: Vec::new(),
        }
    }
}

/// Programme, die nie als „grafisch“ umgeleitet werden.
const NEVER_WRAP: &[&str] = &[
    "tuilnux",
    "tuilnux-session",
    "tsettings",
    "tfiles",
    "ttop",
    "tedit",
    "tlogin",
    "asciia",
    "sh",
    "bash",
    "zsh",
    "fish",
    "env",
    "sudo",
    "doas",
    "pkexec",
    "flatpak",
    "xdg-open",
    "gio",
    "exec",
    "command",
    "builtin",
    "test",
    "true",
    "false",
    "cd",
    "ls",
    "launch",
    "open",
    // Hilfsprogramme mit .desktop-Datei, die man nie selbst als Fenster startet
    "Xwayland",
    "emacs",
    "git",
];

fn name_ok(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('-')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "_.+-".contains(c))
}

fn env_name_ok(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// `NAME=WERT` zerlegen.
pub fn split_pair(entry: &str) -> Option<(&str, &str)> {
    let (name, value) = entry.split_once('=')?;
    Some((name.trim(), value.trim()))
}

impl BashSettings {
    pub fn validate(&self) -> Result<()> {
        let texts = [
            ("bash.welcome_file", &self.welcome_file),
            ("bash.welcome_text", &self.welcome_text),
            ("bash.prompt_custom", &self.prompt_custom),
            ("bash.prompt_symbol", &self.prompt_symbol),
            ("bash.editor", &self.editor),
        ];
        for (name, value) in texts {
            if value.chars().any(char::is_control) {
                bail!("{name} enthält Steuerzeichen");
            }
        }
        for (name, value) in [
            ("bash.prompt_user_color", &self.prompt_user_color),
            ("bash.prompt_path_color", &self.prompt_path_color),
        ] {
            if let Some(v) = value
                && parse_hex(v).is_none()
            {
                bail!("{name} = \"{v}\" ist keine Farbe im Format #rrggbb");
            }
        }
        for entry in &self.aliases {
            match split_pair(entry) {
                Some((name, value)) if name_ok(name) && !value.chars().any(char::is_control) => {}
                _ => bail!("Alias „{entry}“: erwartet NAME=BEFEHL"),
            }
        }
        for entry in &self.env {
            match split_pair(entry) {
                Some((name, value))
                    if env_name_ok(name) && !value.chars().any(char::is_control) => {}
                _ => bail!("Umgebungsvariable „{entry}“: erwartet NAME=WERT"),
            }
        }
        for dir in &self.path {
            if dir.is_empty() || dir.chars().any(|c| c.is_control() || c == ':') {
                bail!("PATH-Eintrag „{dir}“ ist ungültig");
            }
        }
        for app in self.gui_apps.iter().chain(&self.gui_exclude) {
            if !name_ok(app) {
                bail!("Programmname „{app}“ ist ungültig");
            }
        }
        if self.history_size == 0 {
            bail!("bash.history_size muss größer als 0 sein");
        }
        Ok(())
    }
}

/// Text in einfachen Anführungszeichen für die Shell.
pub fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// Pfad mit `~/` als Shell-Ausdruck mit `$HOME`.
fn sh_path(p: &str) -> String {
    match p.strip_prefix("~/") {
        Some(rest) => format!("\"$HOME\"/{}", sh_quote(rest)),
        None if p == "~" => "\"$HOME\"".into(),
        None => sh_quote(p),
    }
}

/// Wert in doppelten Anführungszeichen: `$VAR` wird ersetzt, `` ` `` und `"` nicht.
fn sh_double(s: &str) -> String {
    if let Some(rest) = s.strip_prefix("~/") {
        return format!("\"$HOME\"{}", sh_double(&format!("/{rest}")));
    }
    let mut out = String::from("\"");
    for c in s.chars() {
        if matches!(c, '"' | '`' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('"');
    out
}

/// Farbe für PS1 (in `\[ \]`, damit Bash die Breite richtig zählt).
fn ps1_color(rgb: Rgb) -> String {
    format!(r"\[\e[38;2;{};{};{}m\]", rgb[0], rgb[1], rgb[2])
}

const PS1_RESET: &str = r"\[\e[0m\]";

/// Farbe als `$'…'`-Ausdruck mit readline-Markierungen (für Variablen in PS1).
fn ansi_color(rgb: Rgb) -> String {
    format!(r"$'\001\e[38;2;{};{};{}m\002'", rgb[0], rgb[1], rgb[2])
}

const ANSI_RESET: &str = r"$'\001\e[0m\002'";

struct PromptColors {
    user: Rgb,
    path: Rgb,
    git: Rgb,
    error: Rgb,
    dim: Rgb,
    symbol: Rgb,
}

fn prompt_colors(s: &Settings) -> PromptColors {
    let p: Palette = s.palette();
    let b = &s.bash;
    PromptColors {
        user: b
            .prompt_user_color
            .as_deref()
            .and_then(parse_hex)
            .unwrap_or(p.accent),
        path: b
            .prompt_path_color
            .as_deref()
            .and_then(parse_hex)
            .unwrap_or(p.accent2),
        git: p.warn,
        error: p.error,
        dim: p.dim,
        symbol: p.accent,
    }
}

impl PathStyle {
    fn escape(self) -> &'static str {
        match self {
            Self::Short => r"\W",
            Self::Full | Self::Trimmed => r"\w",
        }
    }
}

/// Fester Teil des Prompts vor Git/Status (ohne Titel).
fn prompt_head(s: &Settings, c: &PromptColors) -> String {
    let b = &s.bash;
    let mut out = String::new();
    if b.prompt_time {
        let _ = write!(out, r"{}\A{PS1_RESET} ", ps1_color(c.dim));
    }
    let who = match (b.prompt_user, b.prompt_host) {
        (true, true) => r"\u@\h",
        (true, false) => r"\u",
        (false, true) => r"\h",
        (false, false) => "",
    };
    match b.prompt {
        PromptStyle::Classic => {
            let _ = write!(out, "[");
            if !who.is_empty() {
                let _ = write!(out, "{}{who}{PS1_RESET} ", ps1_color(c.user));
            }
            let _ = write!(out, r"{}\W{PS1_RESET}", ps1_color(c.path));
        }
        PromptStyle::Minimal => {
            let _ = write!(
                out,
                "{}{}{PS1_RESET}",
                ps1_color(c.path),
                b.path_style.escape()
            );
        }
        _ => {
            if !who.is_empty() {
                let _ = write!(out, "{}{who}{PS1_RESET} ", ps1_color(c.user));
            }
            let _ = write!(
                out,
                "{}{}{PS1_RESET}",
                ps1_color(c.path),
                b.path_style.escape()
            );
        }
    }
    out
}

/// Ende des Prompts (Symbol).
fn prompt_tail(s: &Settings, c: &PromptColors) -> String {
    let b = &s.bash;
    let symbol = ps1_escape(&b.prompt_symbol);
    match b.prompt {
        PromptStyle::Classic => r"]\$ ".into(),
        PromptStyle::TwoLine => format!(r"\n{}{symbol}{PS1_RESET} ", ps1_color(c.symbol)),
        _ => format!(" {}{symbol}{PS1_RESET} ", ps1_color(c.symbol)),
    }
}

/// Text für PS1, in dem `\` und `$` wörtlich gemeint sind.
fn ps1_escape(s: &str) -> String {
    s.replace('\\', r"\\")
        .replace('$', r"\$")
        .replace('`', r"\`")
}

/// Shell-Code, der PS1 setzt (leer bei [`PromptStyle::System`]).
pub fn prompt_code(s: &Settings) -> String {
    let b = &s.bash;
    let c = prompt_colors(s);
    let title = if b.terminal_title {
        r"\[\e]0;\u@\h: \w\a\]"
    } else {
        ""
    };
    let mut out = String::new();
    if b.path_style == PathStyle::Trimmed {
        out.push_str("PROMPT_DIRTRIM=2\n");
    }
    match b.prompt {
        PromptStyle::System => return String::new(),
        PromptStyle::Custom => {
            let _ = writeln!(
                out,
                "PS1={}",
                sh_quote(&format!("{title}{}", b.prompt_custom))
            );
            return out;
        }
        _ => {}
    }
    let head = format!("{title}{}", prompt_head(s, &c));
    let tail = prompt_tail(s, &c);
    let dynamic = b.prompt_git || b.prompt_status;
    if !dynamic {
        let _ = writeln!(out, "PS1={}", sh_quote(&format!("{head}{tail}")));
        return out;
    }
    // Git-Zweig und Status stehen in einer Variablen, auf die PS1 nur verweist:
    // So wird ein Zweigname wie `$(befehl)` nie als Prompt-Code ausgewertet.
    // \001/\002 markieren für readline unsichtbare Farbcodes.
    out.push_str("_tuilnux_prompt() {\n    local s=$?\n    _tuilnux_extra=''\n");
    if b.prompt_git {
        let _ = writeln!(
            out,
            "    local branch\n    \
             if branch=$(git symbolic-ref --short HEAD 2>/dev/null || git rev-parse --short HEAD 2>/dev/null); then\n        \
             _tuilnux_extra+={}\" ($branch)\"{}\n    \
             fi",
            ansi_color(c.git),
            ANSI_RESET
        );
    }
    if b.prompt_status {
        let _ = writeln!(
            out,
            "    (( s )) && _tuilnux_extra+={}\" ✗$s\"{}",
            ansi_color(c.error),
            ANSI_RESET
        );
    }
    out.push_str("    return $s\n}\n");
    let _ = writeln!(
        out,
        "PS1={}'${{_tuilnux_extra}}'{}",
        sh_quote(&head),
        sh_quote(&tail)
    );
    out.push_str(
        "[[ ${PROMPT_COMMAND-} == *_tuilnux_prompt* ]] || PROMPT_COMMAND=\"_tuilnux_prompt${PROMPT_COMMAND:+;$PROMPT_COMMAND}\"\n",
    );
    out
}

/// Prompt so, wie er etwa aussieht (für die Vorschau in tsettings): Teile mit Farbe.
pub fn prompt_preview(s: &Settings) -> Vec<(String, Option<Rgb>)> {
    let b = &s.bash;
    let c = prompt_colors(s);
    let user = std::env::var("USER").unwrap_or_else(|_| "user".into());
    let host = std::fs::read_to_string("/etc/hostname")
        .ok()
        .map(|h| h.trim().to_string())
        .filter(|h| !h.is_empty())
        .unwrap_or_else(|| "arch".into());
    let path = match b.path_style {
        PathStyle::Full => "~/Projekte/tuilnux/src",
        PathStyle::Trimmed => "~/…/tuilnux/src",
        PathStyle::Short => "src",
    };
    let who = match (b.prompt_user, b.prompt_host) {
        (true, true) => format!("{user}@{host}"),
        (true, false) => user,
        (false, true) => host,
        (false, false) => String::new(),
    };
    let mut out: Vec<(String, Option<Rgb>)> = Vec::new();
    let mut push = |t: &str, col: Option<Rgb>| out.push((t.to_string(), col));
    match b.prompt {
        PromptStyle::System => {
            push("(PS1 aus ~/.bashrc)", Some(c.dim));
            return out;
        }
        PromptStyle::Custom => {
            push(&b.prompt_custom, None);
            return out;
        }
        _ => {}
    }
    if b.prompt_time {
        push("12:34 ", Some(c.dim));
    }
    match b.prompt {
        PromptStyle::Classic => {
            push("[", None);
            if !who.is_empty() {
                push(&who, Some(c.user));
                push(" ", None);
            }
            push("src", Some(c.path));
        }
        PromptStyle::Minimal => push(path, Some(c.path)),
        _ => {
            if !who.is_empty() {
                push(&who, Some(c.user));
                push(" ", None);
            }
            push(path, Some(c.path));
        }
    }
    if b.prompt != PromptStyle::Classic {
        if b.prompt_git {
            push(" (main)", Some(c.git));
        }
        if b.prompt_status {
            push(" ✗1", Some(c.error));
        }
    }
    match b.prompt {
        PromptStyle::Classic => push("]$ ", None),
        PromptStyle::TwoLine => {
            push("\n", None);
            push(&b.prompt_symbol, Some(c.symbol));
            push(" ", None);
        }
        _ => {
            push(" ", None);
            push(&b.prompt_symbol, Some(c.symbol));
            push(" ", None);
        }
    }
    out
}

/// Eigene Ergänzungen, die zuletzt geladen werden.
pub fn local_bashrc() -> Option<PathBuf> {
    Some(crate::config_dir()?.join("bashrc.local"))
}

/// Das komplette Skript für `eval "$(tuilnux shell-init bash)"`.
/// `gui_apps`: Programme, die im Hintergrund starten (siehe [`gui_programs`]).
pub fn bash_init(s: &Settings, gui_apps: &[String]) -> String {
    let b = &s.bash;
    let mut out = String::from(
        "# TUILNUX-Shell – erzeugt aus ~/.config/tuilnux/tuilnux.toml (tsettings → Shell)\n\
         # Eigene Ergänzungen: ~/.config/tuilnux/bashrc.local\n",
    );

    out.push_str("\n# Verlauf\n");
    let _ = writeln!(out, "HISTSIZE={0}\nHISTFILESIZE={0}", b.history_size);
    if b.history_ignore_dups {
        out.push_str("HISTCONTROL=ignoreboth:erasedups\n");
    }
    out.push_str("shopt -s histappend checkwinsize\n");
    if b.history_shared {
        out.push_str(
            "_tuilnux_history() { history -a; history -n; }\n\
             [[ ${PROMPT_COMMAND-} == *_tuilnux_history* ]] || PROMPT_COMMAND=\"_tuilnux_history${PROMPT_COMMAND:+;$PROMPT_COMMAND}\"\n",
        );
    }

    if b.completion {
        out.push_str(
            "\n# Tab-Vervollständigung\n\
             if ! declare -F _init_completion >/dev/null && [[ -r /usr/share/bash-completion/bash_completion ]]; then\n    \
             . /usr/share/bash-completion/bash_completion\n\
             fi\n",
        );
    }

    if !b.path.is_empty() {
        out.push_str("\n# PATH\n");
        for dir in &b.path {
            let _ = writeln!(
                out,
                "_tuilnux_dir={}; case \":$PATH:\" in *\":$_tuilnux_dir:\"*) ;; *) PATH=\"$_tuilnux_dir:$PATH\" ;; esac",
                sh_path(dir)
            );
        }
        out.push_str("unset _tuilnux_dir\nexport PATH\n");
    }

    out.push_str("\n# Umgebung\n");
    if !b.editor.trim().is_empty() {
        let _ = writeln!(
            out,
            "export EDITOR={0} VISUAL={0}",
            sh_quote(b.editor.trim())
        );
    }
    for entry in &b.env {
        if let Some((name, value)) = split_pair(entry) {
            let _ = writeln!(out, "export {name}={}", sh_double(value));
        }
    }

    out.push_str("\n# Aliase\n");
    if b.color_aliases {
        out.push_str(
            "alias ls='ls --color=auto' grep='grep --color=auto' diff='diff --color=auto' ip='ip -color=auto'\n",
        );
    }
    for entry in &b.aliases {
        if let Some((name, value)) = split_pair(entry) {
            let _ = writeln!(out, "alias {name}={}", sh_quote(value));
        }
    }

    let prompt = prompt_code(s);
    if !prompt.is_empty() {
        out.push_str("\n# Prompt\n");
        out.push_str(&prompt);
    }

    if b.welcome {
        out.push_str("\n# Begrüßung\n");
        let condition = if b.welcome_in_windows {
            "-z ${TUILNUX_NO_WELCOME-}"
        } else {
            "-z ${TUILNUX_NO_WELCOME-} && ${TUILNUX_WINDOW:-0} == 0"
        };
        let file = if b.welcome_file.trim().is_empty() {
            "\"${XDG_CONFIG_HOME:-$HOME/.config}/tuilnux/welcome.ASCIIA\"".to_string()
        } else {
            sh_path(b.welcome_file.trim())
        };
        let _ = writeln!(
            out,
            "if [[ {condition} ]]; then\n    \
             _tuilnux_welcome={file}\n    \
             [[ -f $_tuilnux_welcome ]] || _tuilnux_welcome=\"${{_tuilnux_share:-/usr/share/tuilnux}}/welcome.ASCIIA\"\n    \
             [[ -f $_tuilnux_welcome ]] && command -v asciia >/dev/null && asciia \"$_tuilnux_welcome\" --once"
        );
        if !b.welcome_text.trim().is_empty() {
            let _ = writeln!(
                out,
                "    printf '%s\\n' {}",
                sh_quote(b.welcome_text.trim())
            );
        }
        out.push_str("    unset _tuilnux_welcome\nfi\n");
    }

    if b.gui_background {
        out.push_str(
            "\n# Grafische Programme starten im Hintergrund, die Shell bleibt frei.\n\
             # Ausgabe ansehen: tab --shell PROGRAMM · direkt starten: command PROGRAMM\n\
             _tuilnux_gui() {\n    \
             if [[ -n ${TUILNUX_SOCK-} && -n ${WAYLAND_DISPLAY-} && -t 1 ]]; then\n        \
             tuilnux launch -- \"$@\"\n    \
             else\n        \
             command \"$@\"\n    \
             fi\n\
             }\n\
             launch() { _tuilnux_gui \"$@\"; }\n",
        );
        if !gui_apps.is_empty() {
            let _ = writeln!(out, "unalias {} 2>/dev/null", gui_apps.join(" "));
        }
        for app in gui_apps {
            let _ = writeln!(out, "{app}() {{ _tuilnux_gui {app} \"$@\"; }}");
        }
    }

    out.push_str(
        "\n# Eigene Ergänzungen\n\
         [[ -r ${XDG_CONFIG_HOME:-$HOME/.config}/tuilnux/bashrc.local ]] && . \"${XDG_CONFIG_HOME:-$HOME/.config}/tuilnux/bashrc.local\"\n\
         true\n",
    );
    out
}

fn in_path(program: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|paths| std::env::split_paths(&paths).any(|dir| dir.join(program).is_file()))
}

/// Ordner mit `.desktop`-Dateien nach XDG.
fn application_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(data) = dirs::data_dir() {
        dirs.push(data.join("applications"));
    }
    let system = std::env::var("XDG_DATA_DIRS")
        .ok()
        .filter(|d| !d.is_empty())
        .unwrap_or_else(|| "/usr/local/share:/usr/share".into());
    for dir in system.split(':').filter(|d| !d.is_empty()) {
        dirs.push(Path::new(dir).join("applications"));
    }
    dirs.push("/var/lib/flatpak/exports/share/applications".into());
    dirs
}

/// Programmname aus einer `.desktop`-Datei, wenn sie ein grafisches Programm startet.
pub fn desktop_program(content: &str) -> Option<String> {
    let mut in_entry = false;
    let (mut exec, mut terminal, mut app, mut hidden) = (None, false, true, false);
    for line in content.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_entry = line == "[Desktop Entry]";
            continue;
        }
        if !in_entry {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match key.trim() {
            "Exec" => exec = Some(value.trim().to_string()),
            "Terminal" => terminal = value.trim() == "true",
            "Type" => app = value.trim() == "Application",
            "Hidden" => hidden = value.trim() == "true",
            _ => {}
        }
    }
    if terminal || !app || hidden {
        return None;
    }
    let exec = exec?;
    let mut words = exec.split_whitespace().map(|w| w.trim_matches('"'));
    let mut first = words.next()?;
    if first == "env" {
        first = words.find(|w| !w.contains('='))?;
    }
    let name = first.rsplit('/').next()?.to_string();
    name_ok(&name).then_some(name)
}

/// Grafische Programme, die aus der Shell im Hintergrund starten sollen:
/// aus den `.desktop`-Dateien (ohne `Terminal=true`) plus `bash.gui_apps`,
/// ohne `bash.gui_exclude`; nur was im `PATH` liegt.
pub fn gui_programs(b: &BashSettings) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for dir in application_dirs() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_none_or(|e| e != "desktop") {
                continue;
            }
            if let Some(name) = std::fs::read_to_string(&path)
                .ok()
                .and_then(|c| desktop_program(&c))
            {
                names.push(name);
            }
        }
    }
    names.extend(b.gui_apps.iter().cloned());
    names.sort();
    names.dedup();
    names.retain(|n| {
        name_ok(n)
            && !NEVER_WRAP.contains(&n.as_str())
            && !n.starts_with("pinentry")
            && !b.gui_exclude.contains(n)
            && (b.gui_apps.contains(n) || in_path(n))
    });
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quoting() {
        assert_eq!(sh_quote("it's"), r"'it'\''s'");
        assert_eq!(sh_path("~/bin"), "\"$HOME\"/'bin'");
        assert_eq!(sh_path("/opt/x"), "'/opt/x'");
        assert_eq!(sh_double("a\"b`c$HOME"), "\"a\\\"b\\`c$HOME\"");
    }

    #[test]
    fn desktop_files() {
        let firefox = "[Desktop Entry]\nType=Application\nExec=/usr/lib/firefox/firefox %u\n\
                       [Desktop Action new]\nExec=firefox --new-window\n";
        assert_eq!(desktop_program(firefox).as_deref(), Some("firefox"));
        let vim = "[Desktop Entry]\nType=Application\nExec=vim %F\nTerminal=true\n";
        assert_eq!(desktop_program(vim), None);
        let envd = "[Desktop Entry]\nExec=env GDK_BACKEND=x11 gimp %U\n";
        assert_eq!(desktop_program(envd).as_deref(), Some("gimp"));
        assert_eq!(
            desktop_program("[Desktop Entry]\nType=Link\nExec=x\n"),
            None
        );
    }

    #[test]
    fn init_script_reflects_settings() {
        let mut s = Settings::default();
        s.bash.aliases = vec!["gs=git status".into(), "q=echo 'hi'".into()];
        s.bash.env = vec!["GOPATH=$HOME/go".into()];
        s.bash.welcome_text = "Hallo Jonas".into();
        let script = bash_init(&s, &["firefox".into()]);
        assert!(script.contains("alias gs='git status'"));
        assert!(script.contains(r"alias q='echo '\''hi'\'''"));
        assert!(script.contains("export GOPATH=\"$HOME/go\""));
        assert!(script.contains("firefox() { _tuilnux_gui firefox \"$@\"; }"));
        assert!(script.contains("printf '%s\\n' 'Hallo Jonas'"));
        assert!(script.contains("_tuilnux_prompt"));
        assert!(script.contains("HISTSIZE=10000"));
        assert!(script.contains("${TUILNUX_WINDOW:-0} == 0"));

        s.bash.prompt = PromptStyle::Custom;
        s.bash.prompt_custom = r"\u \$ ".into();
        s.bash.terminal_title = false;
        s.bash.welcome = false;
        s.bash.gui_background = false;
        let script = bash_init(&s, &["firefox".into()]);
        assert!(script.contains(r"PS1='\u \$ '"));
        assert!(!script.contains("asciia"));
        assert!(!script.contains("firefox()"));

        s.bash.prompt = PromptStyle::System;
        assert!(!bash_init(&s, &[]).contains("PS1="));
    }

    #[test]
    fn validation() {
        let mut b = BashSettings::default();
        assert!(b.validate().is_ok());
        b.aliases = vec!["kaputt".into()];
        assert!(b.validate().is_err());
        b.aliases = vec!["a b=ls".into()];
        assert!(b.validate().is_err());
        b.aliases.clear();
        b.env = vec!["1X=2".into()];
        assert!(b.validate().is_err());
        b.env = vec!["X=1".into()];
        b.gui_apps = vec!["fire fox".into()];
        assert!(b.validate().is_err());
        b.gui_apps.clear();
        b.prompt_custom = "a\nb".into();
        assert!(b.validate().is_err());
    }

    #[test]
    fn preview_has_colors() {
        let s = Settings::default();
        let parts = prompt_preview(&s);
        let text: String = parts.iter().map(|(t, _)| t.as_str()).collect();
        assert!(text.contains("~/Projekte/tuilnux/src"));
        assert!(text.contains("(main)"));
        assert!(parts.iter().any(|(_, c)| c.is_some()));
    }
}
