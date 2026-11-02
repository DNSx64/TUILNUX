#!/usr/bin/env bash
# TUILNUX-Installer
#
#   bash install.sh              Erstinstallation: grafischer Assistent (tuilnux-setup),
#                                danach/sonst: bauen, installieren und einrichten
#   bash install.sh --yes        ohne Rückfragen
#   bash install.sh --uninstall  alles wieder entfernen und den vorherigen Login herstellen
#   bash install.sh --help       alle Optionen
#
# Als normaler Benutzer ausführen; sudo wird nur für Systemschritte verwendet.
# Arch Linux (und Manjaro, EndeavourOS, CachyOS …): baut ein pacman-Paket
# (dist/arch/PKGBUILD) und installiert es.
# Debian/Ubuntu (apt), Fedora (dnf), openSUSE (zypper): installiert die
# Abhängigkeiten mit der Paketverwaltung, baut mit cargo (Rust >= 1.85) oder lädt
# die fertigen Programme vom GitHub-Release und installiert nach /usr/local.

set -euo pipefail

ROOT=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
STATE_DIR=/var/lib/tuilnux-installer
STATE_FILE=$STATE_DIR/state
GREETD_CONF=/etc/greetd/config.toml
GREETD_BACKUP=/etc/greetd/config.toml.tuilnux-backup
MARK_BEGIN='# >>> tuilnux >>>'
MARK_END='# <<< tuilnux <<<'
BINS=(tuilnux tterm tlogin tfiles tedit ttop tsettings asciia tview wallpaper tuilnux-setup)
MIN_RUST=85

YES=0 DRY=0 UNINSTALL=0 GREETD=1 ENABLE=1 BASHRC=1 TESTS=1 GRAPHICAL=1 MANUAL=0 FONT=1
TUI=auto
SYS_LOCALE='' KEYMAP=''
NVIDIA=auto
SOURCE=auto
NVIDIA_CONF=/etc/modprobe.d/tuilnux-nvidia.conf
BACKEND=auto
LOGIN=greetd
CONSOLE_FONT=${TUILNUX_CONSOLE_FONT-}
VCONSOLE=/etc/vconsole.conf
VCONSOLE_BACKUP=/etc/vconsole.conf.tuilnux-backup
CONSOLE_UNICODE=0

usage() {
    cat <<'EOF'
TUILNUX-Installer

Aufruf: bash install.sh [Optionen]

  --yes, -y            keine Rückfragen (für Skripte)
  --tui                grafischen Assistenten (tuilnux-setup) starten
                       (Standard bei der Erstinstallation auf einem Terminal)
  --text               ohne Assistent im Textmodus installieren
  --lang LOCALE        Systemsprache setzen, z. B. de_DE.UTF-8 (locale-gen + localectl)
  --keymap KEYMAP      Tastaturbelegung der Konsole setzen, z. B. de-latin1
  --backend B          Sitzungs-Backend: auto (Standard), labwc, cage oder tty
  --minimal            labwc, foot, Xwayland und wlr-randr nicht installieren (nur Linux-Konsole)
  --nvidia             NVIDIA-Treiber für Wayland einrichten (nvidia-open-dkms, egl-wayland,
                       nvidia_drm.modeset=1); Standard: automatisch, wenn eine NVIDIA-Karte steckt
  --no-nvidia          NVIDIA-Treiber nicht anfassen
  --login MODUS        Start nach dem Booten:
                         greetd     Anmeldebildschirm tlogin (Standard)
                         autologin  direkt in TUILNUX, ohne Anmeldung (kein greetd)
                         console    Text-Login der Konsole, danach TUILNUX (kein greetd)
                       später ändern: sudo tuilnux-boot autologin|console|greetd|off
  --autologin          kurz für --login autologin
  --no-greetd          keinen Anmeldebildschirm einrichten
  --no-enable          greetd einrichten, aber nicht beim Booten aktivieren
  --no-bashrc          ~/.bashrc nicht anpassen
  --no-font            Konsolenschrift nicht auf Terminus umstellen
                       (Standard ter-v16n, ab 2560 px Breite ter-v32n;
                       eigene Wahl per TUILNUX_CONSOLE_FONT=ter-v32n)
  --no-tests           Tests beim Bauen überspringen
  --manual             auch auf Arch ohne pacman-Paket nach /usr/local installieren
  --source             (nicht Arch) immer mit cargo bauen (Rust >= 1.85 nötig)
  --prebuilt           (nicht Arch) fertige Programme vom GitHub-Release laden
                       Standard: bauen, wenn ein passendes Rust da ist, sonst laden
  --dry-run            nur anzeigen, was passieren würde
  --uninstall          TUILNUX entfernen und den vorherigen Login wiederherstellen
  -h, --help           diese Hilfe
EOF
}

while (($#)); do
    case $1 in
        -y | --yes) YES=1 ;;
        --backend)
            BACKEND=${2-}
            shift
            ;;
        --backend=*) BACKEND=${1#*=} ;;
        --tui) TUI=1 ;;
        --text) TUI=0 ;;
        --lang)
            SYS_LOCALE=${2-}
            shift
            ;;
        --lang=*) SYS_LOCALE=${1#*=} ;;
        --keymap)
            KEYMAP=${2-}
            shift
            ;;
        --keymap=*) KEYMAP=${1#*=} ;;
        --minimal) GRAPHICAL=0 ;;
        --nvidia) NVIDIA=1 ;;
        --no-nvidia) NVIDIA=0 ;;
        --login)
            LOGIN=${2-}
            shift
            ;;
        --login=*) LOGIN=${1#*=} ;;
        --autologin) LOGIN=autologin ;;
        --no-greetd) GREETD=0 ;;
        --no-enable) ENABLE=0 ;;
        --no-bashrc) BASHRC=0 ;;
        --no-font) FONT=0 ;;
        --no-tests) TESTS=0 ;;
        --manual) MANUAL=1 ;;
        --source) SOURCE=1 ;;
        --prebuilt) SOURCE=0 ;;
        --dry-run) DRY=1 ;;
        --uninstall) UNINSTALL=1 ;;
        -h | --help)
            usage
            exit 0
            ;;
        *)
            echo "Unbekannte Option: $1 (siehe --help)" >&2
            exit 2
            ;;
    esac
    shift
done

case $LOGIN in
    greetd) ;;
    autologin | console) GREETD=0 ;;
    *)
        echo "--login: greetd, autologin oder console (nicht '$LOGIN')" >&2
        exit 2
        ;;
esac

if [[ -t 1 ]]; then
    C_INFO=$'\033[1;36m' C_WARN=$'\033[1;33m' C_ERR=$'\033[1;31m' C_OFF=$'\033[0m'
else
    C_INFO='' C_WARN='' C_ERR='' C_OFF=''
fi
# TUILNUX_PROGRESS=1 (Einrichtungsassistent): eigene Schrittmarke, damit die
# "==>"-Zeilen von makepkg den Fortschrittsbalken nicht verfälschen.
if [[ ${TUILNUX_PROGRESS:-} == 1 ]]; then
    say() { printf '##> %s\n' "$*"; }
else
    say() { printf '%s==>%s %s\n' "$C_INFO" "$C_OFF" "$*"; }
fi
warn() { printf '%s==> Warnung:%s %s\n' "$C_WARN" "$C_OFF" "$*" >&2; }
die() {
    printf '%s==> Fehler:%s %s\n' "$C_ERR" "$C_OFF" "$*" >&2
    exit 1
}
have() { command -v "$1" >/dev/null 2>&1; }

# Führt einen Befehl aus oder zeigt ihn im Probelauf nur an.
run() {
    if ((DRY)); then
        printf '    $'
        printf ' %q' "$@"
        printf '\n'
    else
        "$@"
    fi
}
root() { run sudo "$@"; }

# Schreibt stdin als root in eine Datei.
root_write() {
    local mode=$1 dest=$2
    if ((DRY)); then
        printf '    $ sudo tee %q (Modus %s)\n' "$dest" "$mode"
        sed 's/^/      | /'
    else
        sudo mkdir -p "$(dirname "$dest")"
        sudo tee "$dest" >/dev/null
        sudo chmod "$mode" "$dest"
    fi
}

ask() {
    local question=$1 default=$2 answer hint
    ((YES)) && return 0
    [[ $default == j ]] && hint='J/n' || hint='j/N'
    read -r -p "$question [$hint] " answer || answer=
    answer=${answer:-$default}
    [[ $answer == [jJyY]* ]]
}

case $BACKEND in auto | labwc | cage | tty) ;; *) die "--backend muss auto, labwc, cage oder tty sein" ;; esac
[[ -z $SYS_LOCALE || $SYS_LOCALE =~ ^[a-z]{2,3}_[A-Z]{2}\.UTF-8$ ]] || die "--lang erwartet z. B. de_DE.UTF-8"
[[ -z $KEYMAP || $KEYMAP =~ ^[A-Za-z0-9_-]+$ ]] || die "--keymap erwartet z. B. de-latin1"
((GRAPHICAL)) || [[ $NVIDIA != auto ]] || NVIDIA=0

if [[ $(uname -s) != Linux ]]; then
    ((DRY)) || die "TUILNUX läuft nur unter Linux."
    warn "Kein Linux – Probelauf zeigt nur die Schritte."
fi
if ((EUID == 0)) && ((!DRY)); then
    die "Bitte als normaler Benutzer starten (nicht mit sudo) – sudo wird bei Bedarf selbst gefragt."
fi

IS_ARCH=0
have pacman && have makepkg && IS_ARCH=1
((MANUAL)) && IS_ARCH=0
if ((IS_ARCH)); then PREFIX=/usr; else PREFIX=/usr/local; fi
SHARE=$PREFIX/share/tuilnux

# Paketverwaltung: pacman (Arch-Familie), apt, dnf oder zypper.
PM=''
if have pacman; then PM=pacman; fi
if ((!IS_ARCH)); then
    if have apt-get; then PM=apt; elif have dnf; then PM=dnf; elif have zypper; then PM=zypper; fi
fi
# shellcheck source=/dev/null
DISTRO=$( (. /etc/os-release 2>/dev/null && echo "${PRETTY_NAME:-$ID}") || echo Linux)
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' "$ROOT/Cargo.toml" | head -n1)
RELEASE_URL=${TUILNUX_RELEASE_URL:-https://github.com/DNSx64/TUILNUX/releases/download}
PREBUILT=$ROOT/target/prebuilt
BIN_SRC=$ROOT/target/release

# ---------------------------------------------------------------- Zustand

state_get() {
    [[ -r $STATE_FILE ]] || return 0
    sed -n "s/^$1=//p" "$STATE_FILE" | tail -n1
}

state_set() {
    local key=$1 value=$2 current=''
    [[ -r $STATE_FILE ]] && current=$(grep -v "^$key=" "$STATE_FILE" || true)
    printf '%s\n%s=%s\n' "$current" "$key" "$value" | sed '/^$/d' | root_write 644 "$STATE_FILE"
}

current_dm() {
    local link
    link=$(readlink /etc/systemd/system/display-manager.service 2>/dev/null) || return 0
    basename "$link" .service
}

# ---------------------------------------------------------------- Installation

install_packages() {
    local pkgs=()
    have cargo || pkgs+=(rust)
    pacman -Qq base-devel >/dev/null 2>&1 || pkgs+=(base-devel)
    # pam-u2f/libpam-google-authenticator: Sicherheitsschlüssel und Handy-Code (tlogin setup)
    ((GREETD)) && pkgs+=(greetd pam-u2f libpam-google-authenticator)
    ((GRAPHICAL)) && pkgs+=(labwc xorg-xwayland foot ttf-dejavu wlr-randr wl-clipboard)
    if ((NVIDIA == 1)); then
        say "NVIDIA-Karte: installiere den offenen Treiber für Wayland"
        pkgs+=(nvidia-open-dkms nvidia-utils egl-wayland libva-nvidia-driver)
        local kernel
        for kernel in linux linux-lts linux-zen linux-hardened; do
            pacman -Qq "$kernel" >/dev/null 2>&1 && pkgs+=("$kernel-headers")
        done
    fi
    [[ $BACKEND == cage ]] && pkgs+=(cage)
    ((FONT)) && pkgs+=(terminus-font)
    if ((${#pkgs[@]})); then
        say "Installiere Pakete: ${pkgs[*]} (inkl. Systemaktualisierung, Arch erlaubt keine Teil-Upgrades)"
        root pacman -Syu --needed --noconfirm "${pkgs[@]}"
    fi
}

# pacman-Name → Paketname der jeweiligen Distribution ('' = gibt es dort nicht).
pkg_name() {
    case $PM:$1 in
        apt:xwayland | zypper:xwayland) echo xwayland ;;
        dnf:xwayland) echo xorg-x11-server-Xwayland ;;
        apt:pam-u2f) echo libpam-u2f ;;
        zypper:pam-u2f) echo pam_u2f ;;
        apt:libpam-google-authenticator) echo libpam-google-authenticator ;;
        dnf:libpam-google-authenticator) echo google-authenticator ;;
        zypper:libpam-google-authenticator) echo google-authenticator-libpam ;;
        apt:ttf-dejavu) echo fonts-dejavu-core ;;
        dnf:ttf-dejavu) echo dejavu-sans-mono-fonts ;;
        zypper:ttf-dejavu) echo dejavu-fonts ;;
        apt:terminus-font) echo '' ;; # Debian/Ubuntu: Konsolenschrift über console-setup
        dnf:terminus-font) echo terminus-fonts-console ;;
        zypper:terminus-font) echo '' ;; # openSUSE: Terminus nur für X11, keine Konsolenschrift
        apt:base-devel) echo build-essential ;;
        dnf:base-devel | zypper:base-devel) echo gcc ;;
        apt:rust) echo cargo ;;
        dnf:rust | zypper:rust) echo cargo ;;
        *) echo "$1" ;;
    esac
}

pm_install() {
    case $PM in
        apt) root env DEBIAN_FRONTEND=noninteractive apt-get install -y "$@" ;;
        dnf) root dnf install -y "$@" ;;
        zypper) root zypper --non-interactive install "$@" ;;
        *) return 1 ;;
    esac
}

# Installiert alles auf einmal; fehlt ein Paket in den Quellen, einzeln weiter.
install_packages_other() {
    if [[ -z $PM ]]; then
        warn "Keine bekannte Paketverwaltung (apt, dnf, zypper) – Abhängigkeiten bitte selbst installieren: greetd, labwc, foot, Xwayland, wlr-randr."
        return 0
    fi
    local wanted=() pkgs=() p name
    ((GREETD)) && wanted+=(greetd pam-u2f libpam-google-authenticator)
    ((GRAPHICAL)) && wanted+=(labwc xwayland foot ttf-dejavu wlr-randr wl-clipboard)
    [[ $BACKEND == cage ]] && wanted+=(cage)
    ((FONT)) && wanted+=(terminus-font)
    if ((SOURCE == 1)); then
        have cc || wanted+=(base-devel)
        have cargo || wanted+=(rust)
    fi
    ((SOURCE == 0)) && ! have curl && wanted+=(curl)
    for p in "${wanted[@]}"; do
        name=$(pkg_name "$p")
        [[ -n $name ]] && pkgs+=("$name")
    done
    ((${#pkgs[@]})) || return 0
    say "Installiere Pakete mit $PM ($DISTRO): ${pkgs[*]}"
    [[ $PM == apt ]] && { root apt-get update || warn "apt-get update ist fehlgeschlagen"; }
    pm_install "${pkgs[@]}" && return 0
    warn "Nicht alle Pakete auf einmal installierbar – versuche sie einzeln."
    for p in "${pkgs[@]}"; do
        pm_install "$p" || warn "Paket $p ist in den Quellen von $DISTRO nicht verfügbar."
    done
}

rust_ok() {
    have cargo || return 1
    local minor
    minor=$(cd "$ROOT" && rustc --version 2>/dev/null | sed -E 's/^rustc 1\.([0-9]+).*/\1/')
    [[ $minor =~ ^[0-9]+$ ]] && ((minor >= MIN_RUST))
}

# Bauen oder fertige Programme? (nur außerhalb von Arch)
choose_source() {
    [[ $SOURCE == auto ]] || return 0
    if rust_ok; then SOURCE=1; else SOURCE=0; fi
}

# Lädt die statisch gelinkten Programme (musl) des passenden Releases.
fetch_prebuilt() {
    local arch name url
    arch=$(uname -m)
    [[ $arch == x86_64 ]] ||
        die "Fertige Programme gibt es nur für x86_64. Bitte Rust >= 1.$MIN_RUST installieren (https://rustup.rs) und --source verwenden."
    BIN_SRC=$PREBUILT/usr/bin
    if [[ -x $BIN_SRC/tuilnux && $("$BIN_SRC/tuilnux" --version 2>/dev/null) == *"$VERSION"* ]]; then
        say "Fertige Programme $VERSION sind bereits geladen."
        return 0
    fi
    name=tuilnux-$VERSION-x86_64-linux.tar.gz
    url=$RELEASE_URL/v$VERSION/$name
    say "Lade fertige Programme: $url"
    if ((DRY)); then
        printf '    $ curl -fL %q | tar -xz -C %q\n' "$url" "$PREBUILT"
        return 0
    fi
    have curl || die "curl fehlt – bitte installieren oder Rust >= 1.$MIN_RUST mit --source verwenden."
    local tmp
    tmp=$(mktemp -d)
    curl -fL --retry 3 -o "$tmp/$name" "$url" || die "Download fehlgeschlagen: $url"
    if curl -fsL -o "$tmp/$name.sha256" "$url.sha256"; then
        (cd "$tmp" && sha256sum -c "$name.sha256" >/dev/null) || die "Prüfsumme von $name stimmt nicht."
    else
        warn "Keine Prüfsumme gefunden ($url.sha256)."
    fi
    rm -rf "$PREBUILT"
    mkdir -p "$PREBUILT"
    tar -xzf "$tmp/$name" -C "$PREBUILT"
    rm -rf "$tmp"
    [[ -x $BIN_SRC/tuilnux ]] || die "Das Archiv enthält kein usr/bin/tuilnux."
}

has_nvidia() {
    local vendor
    for vendor in /sys/bus/pci/devices/*/vendor; do
        [[ -r $vendor && $(<"$vendor") == 0x10de ]] || continue
        # Nur Grafikkarten (Klasse 0x03xxxx), keine NVIDIA-Audio-/USB-Funktionen.
        [[ $(<"${vendor%/vendor}/class") == 0x03* ]] && return 0
    done
    return 1
}

# NVIDIA unter Wayland braucht Kernel-Modesetting (nvidia_drm.modeset=1).
configure_nvidia() {
    ((NVIDIA == 1)) || return 0
    say "Aktiviere NVIDIA-Modesetting ($NVIDIA_CONF)"
    root_write 644 "$NVIDIA_CONF" <<'EOF'
# TUILNUX: NVIDIA unter Wayland (labwc) braucht Kernel-Modesetting.
options nvidia_drm modeset=1 fbdev=1
EOF
    state_set nvidia_conf 1
    if have mkinitcpio; then
        root mkinitcpio -P || warn "mkinitcpio -P ist fehlgeschlagen – bitte selbst ausführen."
    elif have dracut; then
        root dracut --regenerate-all -f || warn "dracut ist fehlgeschlagen – bitte selbst ausführen."
    elif have update-initramfs; then
        root update-initramfs -u -k all || warn "update-initramfs ist fehlgeschlagen – bitte selbst ausführen."
    fi
    echo "    Nach dem Neustart: tsettings → Grafik & NVIDIA zeigt, ob Modesetting aktiv ist."
}

# Außerhalb von Arch kommt der NVIDIA-Treiber aus den Quellen der Distribution.
nvidia_hint() {
    local hint
    case $PM in
        apt) hint="Debian: sudo apt install nvidia-driver (non-free) · Ubuntu: sudo ubuntu-drivers install" ;;
        dnf) hint="Fedora: RPM Fusion aktivieren, dann sudo dnf install akmod-nvidia" ;;
        zypper) hint="openSUSE: sudo zypper install-new-recommends --repo repo-non-free (oder nvidia-open-driver-G06-signed-kmp-default)" ;;
        *) hint="über die Paketverwaltung der Distribution" ;;
    esac
    have nvidia-smi || warn "NVIDIA-Treiber fehlt noch – $hint"
}

check_rust() {
    ((DRY)) && ! have cargo && return 0
    have cargo || die "cargo fehlt. Arch: sudo pacman -S rust · sonst: https://rustup.rs oder --prebuilt"
    local version minor
    version=$(cd "$ROOT" && rustc --version 2>/dev/null) ||
        die "rustc läuft nicht. Mit rustup: rustup default stable"
    minor=$(sed -E 's/^rustc 1\.([0-9]+).*/\1/' <<<"$version")
    if ! [[ $minor =~ ^[0-9]+$ ]] || ((minor < MIN_RUST)); then
        die "Rust 1.$MIN_RUST oder neuer nötig (gefunden: $version). Aktualisieren: rustup update / pacman -Syu – oder ohne Rust: bash install.sh --prebuilt"
    fi
    say "Rust: $version"
}

install_arch_package() {
    local work
    work=$(mktemp -d)
    # shellcheck disable=SC2064
    trap "rm -rf '$work'" EXIT
    local flags=(-s -f -c --noconfirm)
    ((TESTS)) || flags+=(--nocheck)
    say "Baue das pacman-Paket (das dauert beim ersten Mal einige Minuten) …"
    if ((DRY)); then
        printf '    $ (cd %q && BUILDDIR=… PKGDEST=… makepkg %s)\n' "$ROOT/dist/arch" "${flags[*]}"
    else
        (cd "$ROOT/dist/arch" && BUILDDIR="$work/build" PKGDEST="$work/pkg" makepkg "${flags[@]}")
    fi
    say "Installiere das Paket"
    if ((DRY)); then
        printf '    $ sudo pacman -U --noconfirm %q\n' "$work/pkg/tuilnux-*.pkg.tar.*"
    else
        local pkg
        pkg=$(find "$work/pkg" -name 'tuilnux-*.pkg.tar.*' ! -name '*.sig' | head -n1)
        [[ -n $pkg ]] || die "makepkg hat kein Paket erzeugt."
        sudo pacman -U --noconfirm "$pkg"
    fi
    state_set method pacman
}

install_manual() {
    if ((SOURCE == 0)); then
        fetch_prebuilt
    else
        say "Baue mit cargo …"
        run cargo build --release --locked --workspace --manifest-path "$ROOT/Cargo.toml"
        if ((TESTS)); then
            say "Teste …"
            run cargo test --release --locked --workspace --manifest-path "$ROOT/Cargo.toml"
        fi
    fi
    say "Installiere nach $PREFIX"
    local files=() bin
    for bin in "${BINS[@]}"; do
        root install -Dm755 "$BIN_SRC/$bin" "$PREFIX/bin/$bin"
        files+=("$PREFIX/bin/$bin")
    done
    root install -Dm755 "$ROOT/dist/tuilnux-session" "$PREFIX/bin/tuilnux-session"
    root install -Dm755 "$ROOT/dist/tuilnux-boot" "$PREFIX/bin/tuilnux-boot"
    root install -Dm644 "$ROOT/dist/bashrc.sh" "$SHARE/bashrc.sh"
    root install -Dm644 "$ROOT/dist/greetd/config.toml" "$SHARE/greetd/config.toml"
    root install -Dm644 "$ROOT"/assets/*.ASCIIA "$ROOT"/assets/*.png -t "$SHARE/"
    root install -Dm644 "$ROOT"/crates/tuilnux-i18n/lang/*.toml -t "$SHARE/lang/"
    root install -Dm644 "$ROOT"/dist/labwc/* -t "$SHARE/labwc/"
    files+=("$PREFIX/bin/tuilnux-session" "$PREFIX/bin/tuilnux-boot" "$SHARE/bashrc.sh" "$SHARE/greetd/config.toml")
    local asset
    for asset in "$ROOT"/assets/*.ASCIIA "$ROOT"/assets/*.png; do files+=("$SHARE/$(basename "$asset")"); done
    for asset in "$ROOT"/crates/tuilnux-i18n/lang/*.toml; do files+=("$SHARE/lang/$(basename "$asset")"); done
    for asset in "$ROOT"/dist/labwc/*; do files+=("$SHARE/labwc/$(basename "$asset")"); done
    # Titelknöpfe _ □ x für GTK-Programme (tuilnux-session setzt XDG_CONFIG_DIRS).
    local gtk
    for gtk in gtk-3.0 gtk-4.0; do
        root install -Dm644 "$ROOT/dist/gtk/settings.ini" "$SHARE/xdg/$gtk/settings.ini"
        files+=("$SHARE/xdg/$gtk/settings.ini")
    done
    if [[ ! -e /etc/tuilnux/session.conf ]]; then
        root install -Dm644 "$ROOT/dist/session.conf" /etc/tuilnux/session.conf
        files+=(/etc/tuilnux/session.conf)
    fi
    # Bis 0.2.0 hieß tview „view“ – das kollidierte mit vim.
    if [[ -r $STATE_DIR/manifest ]] && grep -qx "$PREFIX/bin/view" "$STATE_DIR/manifest"; then
        root rm -f "$PREFIX/bin/view"
    fi
    printf '%s\n' "${files[@]}" | root_write 644 "$STATE_DIR/manifest"
    state_set method manual
}

configure_backend() {
    [[ $BACKEND == auto ]] && return 0
    say "Backend: $BACKEND"
    if ((DRY)); then
        printf '    $ sudo sed -i s/^TUILNUX_BACKEND=.*/TUILNUX_BACKEND=%s/ /etc/tuilnux/session.conf\n' "$BACKEND"
    else
        sudo sed -i "s/^TUILNUX_BACKEND=.*/TUILNUX_BACKEND=$BACKEND/" /etc/tuilnux/session.conf
    fi
}

# Terminus kennt alle Rahmenlinien – damit zeichnen tlogin und die tty-Sitzung
# echte Linien statt ASCII-Ersatz. Nur Glyphenbreiten von 8/16 Pixel: bei 10/12
# Pixel breiten Schriften (ter-v20n, ter-v24n) mischt fbcon die Textfarben.
pick_console_font() {
    [[ -n $CONSOLE_FONT ]] && return 0
    local width=0
    [[ -r /sys/class/graphics/fb0/virtual_size ]] && width=$(cut -d, -f1 /sys/class/graphics/fb0/virtual_size)
    if ((width >= 2560)); then CONSOLE_FONT=ter-v32n; else CONSOLE_FONT=ter-v16n; fi
}

configure_console_font() {
    ((FONT)) || return 0
    pick_console_font
    local dir found=0
    for dir in /usr/share/kbd/consolefonts /usr/lib/kbd/consolefonts; do
        [[ -e $dir/$CONSOLE_FONT.psf.gz || -e $dir/$CONSOLE_FONT.psf ]] && found=1
    done
    if ((!found && !DRY)); then
        if [[ $PM == apt ]]; then
            warn "Debian/Ubuntu: Konsolenschrift bleibt (Terminus wählen: sudo dpkg-reconfigure console-setup) – Rahmen auf der Konsole als ASCII."
        elif [[ $PM == zypper ]]; then
            warn "openSUSE: keine Terminus-Konsolenschrift verfügbar – Schrift bleibt unverändert, Rahmen auf der Konsole als ASCII."
        else
            warn "Konsolenschrift $CONSOLE_FONT nicht gefunden (Paket $(PM=${PM:-pacman} pkg_name terminus-font)) – Schrift bleibt unverändert."
        fi
        return 0
    fi
    local current=''
    [[ -r $VCONSOLE ]] && current=$(sed -n 's/^FONT=//p' "$VCONSOLE" | tr -d '"' | tail -1)
    if [[ $current == ter-v20n ]]; then
        # Standard älterer TUILNUX-Versionen ersetzen (Farbfehler, siehe oben).
        say "Konsolenschrift: $current → $CONSOLE_FONT"
        root sed -i "s/^FONT=.*/FONT=$CONSOLE_FONT/" "$VCONSOLE"
        root systemctl restart systemd-vconsole-setup.service 2>/dev/null || true
        state_set console_unicode 1
        CONSOLE_UNICODE=1
        return 0
    fi
    if [[ $current == ter-* ]]; then
        say "Konsolenschrift ist bereits Terminus ($current)."
        state_set console_unicode 1
        CONSOLE_UNICODE=1
        return 0
    fi
    if [[ -n $current ]]; then
        warn "Eigene Konsolenschrift ($current) bleibt – Rahmen auf der Konsole als ASCII."
        return 0
    fi
    say "Konsolenschrift: $CONSOLE_FONT ($VCONSOLE)"
    if [[ -e $VCONSOLE ]]; then
        [[ -e $VCONSOLE_BACKUP ]] || root cp -a "$VCONSOLE" "$VCONSOLE_BACKUP"
    else
        state_set vconsole_created 1
    fi
    { [[ -r $VCONSOLE ]] && cat "$VCONSOLE"; echo "FONT=$CONSOLE_FONT"; } | root_write 644 "$VCONSOLE"
    state_set console_font 1
    state_set console_unicode 1
    CONSOLE_UNICODE=1
    # Sofort auf allen Konsolen anwenden (sonst ab dem nächsten Boot).
    root systemctl restart systemd-vconsole-setup.service 2>/dev/null || true
}

# Benutzer, unter dem greetd den Greeter startet: aus der vorhandenen Konfiguration
# (ohne TUILNUX-Version), sonst der erste vorhandene Systembenutzer.
greeter_user() {
    local conf u
    for conf in "$GREETD_BACKUP" "$GREETD_CONF"; do
        [[ -r $conf ]] || continue
        u=$(sed -n 's/^[[:space:]]*user[[:space:]]*=[[:space:]]*"\([^"]*\)".*/\1/p' "$conf" | head -n1)
        if [[ -n $u ]] && getent passwd "$u" >/dev/null 2>&1; then
            echo "$u"
            return 0
        fi
    done
    for u in greeter _greetd greetd; do
        getent passwd "$u" >/dev/null 2>&1 && { echo "$u"; return 0; }
    done
    return 0
}

configure_greetd() {
    ((GREETD)) || return 0
    if [[ ! -d /etc/greetd ]] && ((!DRY)); then
        warn "greetd ist nicht installiert – Anmeldebildschirm übersprungen."
        return 0
    fi
    local guser
    guser=$(greeter_user)
    if [[ -z $guser ]] && ((!DRY)); then
        warn "Kein Greeter-Benutzer (greeter/_greetd/greetd) gefunden – Anmeldebildschirm übersprungen."
        return 0
    fi
    guser=${guser:-greeter}
    say "Richte den Anmeldebildschirm ein ($GREETD_CONF)"
    if [[ -f $GREETD_CONF ]] && ! grep -q tlogin "$GREETD_CONF" && [[ ! -e $GREETD_BACKUP ]]; then
        root cp -a "$GREETD_CONF" "$GREETD_BACKUP"
    fi
    root install -Dm644 "$SHARE/greetd/config.toml" "$GREETD_CONF"
    if [[ $guser != greeter ]]; then
        # Debian/Ubuntu: _greetd, Fedora: greetd
        root sed -i "s/^user = \"greeter\"/user = \"$guser\"/" "$GREETD_CONF"
    fi
    if ((CONSOLE_UNICODE)) || [[ $(state_get console_unicode) == 1 ]]; then
        # Unicode-Konsolenschrift: tlogin (und die Sitzung) zeichnen echte Linien.
        root sed -i 's/^command = "tlogin /command = "env TUILNUX_ASCII=0 tlogin /' "$GREETD_CONF"
    fi

    ((ENABLE)) || return 0
    local dm
    dm=$(current_dm)
    if [[ $dm == greetd ]]; then
        say "greetd ist bereits aktiv."
        return 0
    fi
    if [[ -n $dm ]]; then
        warn "Aktueller Anmeldemanager: $dm"
        ask "$dm deaktivieren und TUILNUX (greetd) beim Booten verwenden?" n || {
            say "greetd bleibt inaktiv. Später: sudo systemctl disable $dm && sudo systemctl enable greetd"
            return 0
        }
        root systemctl disable "$dm.service"
        state_set previous_dm "$dm"
    else
        ask "TUILNUX-Anmeldung beim Booten aktivieren (ersetzt den Text-Login auf TTY1)?" j || {
            say "greetd bleibt inaktiv. Später: sudo systemctl enable greetd"
            return 0
        }
    fi
    root systemctl enable greetd.service
    state_set greetd_enabled 1
}

# autologin/console: Start ohne greetd über getty + Login-Shell (tuilnux-boot).
configure_boot() {
    [[ $LOGIN == greetd ]] && return 0
    if ((!ENABLE)); then
        say "Startmodus $LOGIN nicht aktiviert (--no-enable). Später: sudo tuilnux-boot $LOGIN"
        return 0
    fi
    local tool
    tool=$(command -v tuilnux-boot 2>/dev/null || echo "$PREFIX/bin/tuilnux-boot")
    [[ -x $tool ]] || tool=$PREFIX/bin/tuilnux-boot
    say "Startmodus: $LOGIN (ohne greetd)"
    root "$tool" "$LOGIN" --user "$(id -un)"
    state_set boot_mode "$LOGIN"
}

configure_locale() {
    if [[ -n $SYS_LOCALE ]]; then
        say "Systemsprache: $SYS_LOCALE"
        if [[ -f /etc/locale.gen ]] && ! grep -q "^$SYS_LOCALE " /etc/locale.gen; then
            root sed -i "s/^#[[:space:]]*\($SYS_LOCALE \)/\1/" /etc/locale.gen
            root locale-gen
        fi
        if have localectl; then
            root localectl set-locale "LANG=$SYS_LOCALE"
        else
            echo "LANG=$SYS_LOCALE" | root_write 644 /etc/locale.conf
        fi
    fi
    if [[ -n $KEYMAP ]]; then
        say "Tastaturbelegung (Konsole): $KEYMAP"
        if have localectl; then
            root localectl set-keymap --no-convert "$KEYMAP" || warn "Keymap $KEYMAP unbekannt"
        else
            warn "localectl fehlt – KEYMAP=$KEYMAP bitte selbst in $VCONSOLE eintragen."
        fi
    fi
}

configure_user() {
    local conf=${XDG_CONFIG_HOME:-$HOME/.config}/tuilnux
    say "Benutzerkonfiguration in $conf"
    run mkdir -p "$conf"
    local asset name
    for asset in "$ROOT"/assets/*.ASCIIA; do
        name=$(basename "$asset")
        [[ -e $conf/$name ]] || run cp "$asset" "$conf/$name"
    done
    if [[ ! -e $conf/tuilnux.toml ]]; then
        if ((DRY)); then
            echo "    \$ (schreibe $conf/tuilnux.toml)"
        else
            cat >"$conf/tuilnux.toml" <<'EOF'
# TUILNUX-Sitzung – alle Einträge sind optional.
# prefix = "ctrl+space"        # Taste für den Manager-Modus (z. B. "ctrl+a", "alt+space", "F12")
# shell = ["/bin/bash"]        # Desktop-Shell, Standard: $SHELL
# autostart = ["ttop"]         # beim Start als Fenster öffnen
EOF
        fi
    fi

    ((BASHRC)) || return 0
    local rc=$HOME/.bashrc
    if [[ -f $rc ]] && grep -qF "$MARK_BEGIN" "$rc"; then
        say "$rc enthält die TUILNUX-Integration bereits."
        return 0
    fi
    say "Ergänze ~/.bashrc (Begrüßung per asciia, Aliase open/files/top/edit)"
    if ((DRY)); then
        echo "    \$ (hänge TUILNUX-Block an $rc an)"
        return 0
    fi
    [[ -f $rc ]] && cp -a "$rc" "$rc.tuilnux-backup"
    cat >>"$rc" <<EOF

$MARK_BEGIN
[ -f $SHARE/bashrc.sh ] && . $SHARE/bashrc.sh
$MARK_END
EOF
}

self_test() {
    ((DRY)) && return 0
    say "Selbsttest"
    local bin missing=0
    # sudo/secure_path lassen /usr/local/bin mitunter weg – Selbsttest unabhängig davon.
    [[ :$PATH: == *":$PREFIX/bin:"* ]] || PATH="$PREFIX/bin:$PATH"
    for bin in "${BINS[@]}" tuilnux-session; do
        if ! have "$bin"; then
            warn "$bin wurde nicht im PATH gefunden"
            missing=1
        fi
    done
    tuilnux --version >/dev/null || die "tuilnux startet nicht."
    asciia --check "$SHARE/welcome.ASCIIA" >/dev/null || warn "welcome.ASCIIA ist ungültig"
    ((missing)) && warn "Ist $PREFIX/bin im PATH?"
    return 0
}

do_install() {
    ((IS_ARCH)) || choose_source
    local how
    if ((IS_ARCH)); then
        how='pacman-Paket'
    elif ((SOURCE == 1)); then
        how="mit cargo gebaut nach $PREFIX"
    else
        how="fertige Programme nach $PREFIX"
    fi
    say "TUILNUX-Installation auf $DISTRO ($how)"
    if [[ $NVIDIA == auto ]]; then
        NVIDIA=0
        if has_nvidia; then
            if { ! ((IS_ARCH)) || pacman -Qq 2>/dev/null | grep -qE '^nvidia(-open)?(-dkms)?$'; } &&
                { [[ -e $NVIDIA_CONF ]] || grep -qrs 'nvidia.drm.modeset=1\|nvidia_drm modeset=1' /etc/modprobe.d /proc/cmdline; }; then
                say "NVIDIA-Treiber mit Modesetting ist bereits eingerichtet."
            elif ask "NVIDIA-Grafikkarte gefunden – für Wayland einrichten (Treiber, nvidia_drm.modeset=1)?" j; then
                NVIDIA=1
            fi
        fi
    fi
    if ((!DRY)); then
        sudo -v || die "sudo wird für die Installation benötigt."
    fi
    if ((IS_ARCH)); then
        install_packages
        check_rust
    else
        install_packages_other
        ((SOURCE == 1)) && check_rust
        ((NVIDIA == 1)) && nvidia_hint
    fi
    ((DRY)) || sudo mkdir -p "$STATE_DIR"
    if ((IS_ARCH)); then install_arch_package; else install_manual; fi
    configure_backend
    configure_console_font
    configure_locale
    configure_nvidia
    configure_greetd
    configure_boot
    configure_user
    self_test

    cat <<EOF

${C_INFO}TUILNUX ist installiert.${C_OFF}

  Sofort ausprobieren:   tuilnux-session   (auf einer TTY, z. B. Strg+Alt+F2)
                         tuilnux           (in einem beliebigen Terminal)
  Bedienung:             Strg+Leertaste, dann ? für die Hilfe
  Backend ändern:        /etc/tuilnux/session.conf  (auto | labwc | cage | tty)
  Entfernen:             bash install.sh --uninstall
EOF
    case $(state_get boot_mode) in
        autologin) echo "  Beim nächsten Neustart startet TUILNUX direkt (ohne Anmeldung) auf TTY1." ;;
        console) echo "  Beim nächsten Neustart: auf TTY1 anmelden, danach startet TUILNUX." ;;
    esac
    echo "  Startmodus ändern:     sudo tuilnux-boot autologin|console|greetd|off"
    if [[ $(state_get greetd_enabled) == 1 ]]; then
        echo "  Beim nächsten Neustart erscheint die TUILNUX-Anmeldung (tlogin) auf TTY1."
        echo "  Notfalls: Strg+Alt+F2, anmelden, bash install.sh --uninstall"
    fi
}

# ---------------------------------------------------------------- Deinstallation

do_uninstall() {
    say "Entferne TUILNUX"
    ((DRY)) || sudo -v || die "sudo wird benötigt."

    if [[ $(state_get greetd_enabled) == 1 ]]; then
        root systemctl disable greetd.service || true
        local dm
        dm=$(state_get previous_dm)
        if [[ -n $dm ]]; then
            say "Aktiviere wieder: $dm"
            root systemctl enable "$dm.service" || warn "$dm konnte nicht aktiviert werden"
        fi
    fi
    if [[ -e /etc/tuilnux/boot.conf || -e /etc/profile.d/tuilnux-autostart.sh ]]; then
        say "Entferne den automatischen Start (tuilnux-boot)"
        if have tuilnux-boot; then
            root tuilnux-boot off || warn "tuilnux-boot off fehlgeschlagen"
        elif [[ -x $PREFIX/bin/tuilnux-boot ]]; then
            root "$PREFIX/bin/tuilnux-boot" off || warn "tuilnux-boot off fehlgeschlagen"
        fi
    fi
    if [[ -e /etc/pam.d/greetd.tuilnux-orig || -e /etc/tuilnux/login.toml ]]; then
        say "Setze die Anmelde-Sicherheit zurück (PAM, faillock, login.toml)"
        if have tlogin; then
            root tlogin setup --reset --yes || warn "tlogin setup --reset fehlgeschlagen"
        elif [[ -e /etc/pam.d/greetd.tuilnux-orig ]]; then
            root mv -f /etc/pam.d/greetd.tuilnux-orig /etc/pam.d/greetd
        fi
    fi
    if [[ -e $GREETD_BACKUP ]]; then
        say "Stelle $GREETD_CONF wieder her"
        root mv -f "$GREETD_BACKUP" "$GREETD_CONF"
    elif [[ -f $GREETD_CONF ]] && grep -q tlogin "$GREETD_CONF"; then
        warn "$GREETD_CONF verweist noch auf tlogin – bitte anpassen, bevor greetd wieder genutzt wird."
    fi

    if [[ $(state_get console_font) == 1 ]]; then
        if [[ -e $VCONSOLE_BACKUP ]]; then
            say "Stelle $VCONSOLE wieder her"
            root mv -f "$VCONSOLE_BACKUP" "$VCONSOLE"
        elif [[ $(state_get vconsole_created) == 1 ]]; then
            root rm -f "$VCONSOLE"
        fi
    fi

    if [[ $(state_get nvidia_conf) == 1 ]]; then
        say "Entferne $NVIDIA_CONF (die NVIDIA-Pakete bleiben installiert)"
        root rm -f "$NVIDIA_CONF"
    fi

    local method
    method=$(state_get method)
    if [[ $method == pacman ]] || { [[ -z $method ]] && pacman -Qq tuilnux >/dev/null 2>&1; }; then
        root pacman -R --noconfirm tuilnux
    elif [[ -r $STATE_DIR/manifest ]]; then
        local file
        while IFS= read -r file; do
            [[ -n $file ]] && root rm -f "$file"
        done <"$STATE_DIR/manifest"
        root rmdir --ignore-fail-on-non-empty "$SHARE/greetd" "$SHARE/labwc" "$SHARE/lang" "$SHARE" /etc/tuilnux 2>/dev/null || true
    else
        warn "Keine Installation gefunden."
    fi

    local rc=$HOME/.bashrc
    if [[ -f $rc ]] && grep -qF "$MARK_BEGIN" "$rc"; then
        say "Entferne den TUILNUX-Block aus ~/.bashrc"
        if ((DRY)); then
            echo "    \$ (entferne Block zwischen den Markierungen)"
        else
            cp -a "$rc" "$rc.tuilnux-backup"
            awk -v b="$MARK_BEGIN" -v e="$MARK_END" '
                $0 == b { skip = 1; if (n && lines[n] == "") n--; next }
                $0 == e { skip = 0; next }
                !skip { lines[++n] = $0 }
                END { for (i = 1; i <= n; i++) print lines[i] }
            ' "$rc.tuilnux-backup" >"$rc"
        fi
    fi

    root rm -rf "$STATE_DIR"
    say "Fertig. Deine Einstellungen in ~/.config/tuilnux bleiben erhalten."
}

# ---------------------------------------------------------------- Assistent

# Erstinstallation auf einem Terminal: Rust bereitstellen, tuilnux-setup bauen
# und starten. Der Assistent ruft danach `install.sh --text …` mit der Auswahl auf.
want_wizard() {
    ((UNINSTALL || DRY)) && return 1
    [[ $TUI == 0 ]] && return 1
    [[ -t 0 && -t 1 ]] || return 1
    [[ $TUI == 1 ]] && return 0
    ((YES)) && return 1
    [[ ! -e $STATE_FILE ]]
}

run_wizard() {
    say "Starte den grafischen Installationsassistenten (bash install.sh --text überspringt ihn)"
    sudo -v || die "sudo wird für die Installation benötigt."
    if ((IS_ARCH)) && { ! have cargo || ! pacman -Qq base-devel >/dev/null 2>&1; }; then
        say "Installiere Rust und base-devel für den Assistenten"
        sudo pacman -Syu --needed --noconfirm rust base-devel
    fi
    if ((!IS_ARCH)); then
        choose_source
        if ((SOURCE == 0)); then
            if ! have curl; then
                say "Installiere curl für den Download"
                [[ $PM == apt ]] && { root apt-get update || true; }
                pm_install curl || die "curl konnte nicht installiert werden."
            fi
            fetch_prebuilt
            exec "$BIN_SRC/tuilnux-setup" --root "$ROOT"
        fi
    fi
    have cargo || die "cargo fehlt. Arch: sudo pacman -S rust · sonst: https://rustup.rs"
    say "Baue den Assistenten (tuilnux-setup) …"
    if ! cargo build --release --locked -p tuilnux-setup --manifest-path "$ROOT/Cargo.toml"; then
        warn "Assistent konnte nicht gebaut werden – weiter im Textmodus."
        return 1
    fi
    exec "$ROOT/target/release/tuilnux-setup" --root "$ROOT"
}

if ((UNINSTALL)); then
    do_uninstall
else
    if want_wizard; then run_wizard || true; fi
    do_install
fi
