# TUILNUX-Shell-Integration – wird aus ~/.bashrc geladen:
#   [ -f /usr/share/tuilnux/bashrc.sh ] && . /usr/share/tuilnux/bashrc.sh
#
# Prompt (PS1), Begrüßung, Verlauf, Aliase, PATH und welche grafischen Programme
# im Hintergrund starten, stellt man in tsettings → Shell ein (tuilnux.toml,
# Abschnitt [bash]). Eigene Ergänzungen: ~/.config/tuilnux/bashrc.local
# Erzeugtes Skript ansehen: tuilnux shell-init

[[ $- == *i* ]] || return 0

_tuilnux_share=$(dirname -- "${BASH_SOURCE[0]}")

if command -v tuilnux >/dev/null && _tuilnux_init=$(tuilnux shell-init bash 2>/dev/null); then
    eval "$_tuilnux_init"
elif [[ -z ${TUILNUX_NO_WELCOME-} && ${TUILNUX_WINDOW:-0} == 0 ]] && command -v asciia >/dev/null; then
    # Rückfall ohne tuilnux: nur die Begrüßung.
    _tuilnux_welcome="${XDG_CONFIG_HOME:-$HOME/.config}/tuilnux/welcome.ASCIIA"
    [[ -f $_tuilnux_welcome ]] || _tuilnux_welcome="$_tuilnux_share/welcome.ASCIIA"
    [[ -f $_tuilnux_welcome ]] && asciia "$_tuilnux_welcome" --once
    unset _tuilnux_welcome
fi
unset _tuilnux_init

# In der TUILNUX-Sitzung öffnen sich Programme als Fenster.
if [[ -n ${TUILNUX_SOCK-} ]]; then
    # Die Fenster-ID (Ausgabe von "tuilnux open") braucht man interaktiv nicht.
    unalias open files top edit settings tab 2>/dev/null
    open() { tuilnux open -- "$@" >/dev/null; }
    files() { open tfiles "$@"; }
    top() { open ttop "$@"; }
    edit() { open tedit "$@"; }
    settings() { open tsettings "$@"; }
    # tab NAME – minimierten Task wiederherstellen (tab ohne NAME listet die Tasks).
    # tab --shell NAME – Ausgabe/Shell eines im Hintergrund gestarteten Programms.
    tab() { tuilnux tab "$@"; }
    _tuilnux_tab() {
        local IFS=$'\n' cur=${COMP_WORDS[COMP_CWORD]}
        if [[ $cur == -* ]]; then
            mapfile -t COMPREPLY < <(compgen -W '--shell --names' -- "$cur")
        elif [[ ${COMP_WORDS[1]-} == --shell ]]; then
            mapfile -t COMPREPLY < <(compgen -W "$(tuilnux tab --shell --names 2>/dev/null)" -- "$cur")
        else
            mapfile -t COMPREPLY < <(compgen -W "$(tuilnux tab --names 2>/dev/null)" -- "$cur")
        fi
    }
    complete -F _tuilnux_tab tab
else
    alias files='tfiles'
    alias top='ttop'
    alias edit='tedit'
    alias settings='tsettings'
fi

# Bilder im Terminal: "bild DATEI" zeigt eine Datei (Endung egal),
# "bild" ohne Argument bzw. Alt+V fügt ein kopiertes Bild oder eine kopierte Bilddatei ein.
if command -v tview >/dev/null; then
    bild() {
        if (($#)); then tview "$@"; else tview --paste; fi
    }
    alias einfuegen='tview --paste'
    bind -x '"\ev": tview --paste' 2>/dev/null
fi

: "${EDITOR:=tedit}"
export EDITOR
unset _tuilnux_share