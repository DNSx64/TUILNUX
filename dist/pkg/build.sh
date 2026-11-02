#!/usr/bin/env bash
# Baut statisch gelinkte Programme (musl) und daraus die Pakete für andere
# Distributionen:
#   tuilnux_<ver>_amd64.deb             Debian, Ubuntu, Mint, Pop!_OS …
#   tuilnux-<ver>-1.x86_64.rpm          Fedora, openSUSE, RHEL-Familie …
#   tuilnux-<ver>-x86_64-linux.tar.gz   alle anderen (Inhalt: usr/ und etc/)
#
# Aufruf (auf Linux, braucht das Rust-Target x86_64-unknown-linux-musl;
# Arch: sudo pacman -S rust-musl musl):
#   bash dist/pkg/build.sh             bauen und packen → target/packages/
#   bash dist/pkg/build.sh --no-build  vorhandene musl-Programme packen
set -euo pipefail

ROOT=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)
PKGDIR=$ROOT/dist/pkg
TARGET=x86_64-unknown-linux-musl
OUT=$ROOT/target/packages
STAGE=$OUT/stage
NFPM_VERSION=2.48.0
BINS=(tuilnux tterm tlogin tfiles tedit ttop tsettings asciia tview wallpaper tuilnux-setup)
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' "$ROOT/Cargo.toml" | head -n1)

build=1
[[ ${1-} == --no-build ]] && build=0

if ((build)); then
    echo "==> Baue $VERSION für $TARGET"
    cargo build --release --locked --workspace --target "$TARGET" --manifest-path "$ROOT/Cargo.toml"
fi
REL=$ROOT/target/$TARGET/release

echo "==> Stelle die Dateien zusammen"
rm -rf "$OUT"
mkdir -p "$STAGE"
for bin in "${BINS[@]}"; do
    install -Dm755 "$REL/$bin" "$STAGE/usr/bin/$bin"
    strip "$STAGE/usr/bin/$bin"
done
if file "$STAGE/usr/bin/tuilnux" | grep -q 'dynamically linked'; then
    echo "Fehler: tuilnux ist nicht statisch gelinkt" >&2
    exit 1
fi
cd "$ROOT"
install -Dm755 dist/tuilnux-session "$STAGE/usr/bin/tuilnux-session"
install -Dm755 dist/tuilnux-boot "$STAGE/usr/bin/tuilnux-boot"
install -Dm644 dist/session.conf "$STAGE/etc/tuilnux/session.conf"
install -Dm644 dist/bashrc.sh "$STAGE/usr/share/tuilnux/bashrc.sh"
install -Dm644 assets/*.ASCIIA assets/*.png -t "$STAGE/usr/share/tuilnux/"
install -Dm644 crates/tuilnux-i18n/lang/*.toml -t "$STAGE/usr/share/tuilnux/lang/"
install -Dm644 dist/labwc/* -t "$STAGE/usr/share/tuilnux/labwc/"
install -Dm644 dist/gtk/settings.ini "$STAGE/usr/share/tuilnux/xdg/gtk-3.0/settings.ini"
install -Dm644 dist/gtk/settings.ini "$STAGE/usr/share/tuilnux/xdg/gtk-4.0/settings.ini"
install -Dm644 dist/greetd/config.toml "$STAGE/usr/share/tuilnux/greetd/config.toml"
install -Dm644 README.md docs/ANLEITUNG.md -t "$STAGE/usr/share/doc/tuilnux/"
install -Dm644 LICENSE "$STAGE/usr/share/doc/tuilnux/copyright"
install -Dm644 LICENSE "$STAGE/usr/share/licenses/tuilnux/LICENSE"

tarball=tuilnux-$VERSION-x86_64-linux.tar.gz
echo "==> $tarball"
tar --owner=0 --group=0 --sort=name -czf "$OUT/$tarball" -C "$STAGE" usr etc
(cd "$OUT" && sha256sum "$tarball" >"$tarball.sha256")

nfpm=$(command -v nfpm || true)
if [[ -z $nfpm ]]; then
    nfpm=$ROOT/target/tools/nfpm
    if [[ ! -x $nfpm ]]; then
        echo "==> Lade nfpm $NFPM_VERSION"
        mkdir -p "$ROOT/target/tools"
        base=https://github.com/goreleaser/nfpm/releases/download/v$NFPM_VERSION
        archive=nfpm_${NFPM_VERSION}_Linux_x86_64.tar.gz
        curl -fsSL -o "$ROOT/target/tools/$archive" "$base/$archive"
        curl -fsSL -o "$ROOT/target/tools/checksums.txt" "$base/checksums.txt"
        (cd "$ROOT/target/tools" && grep " $archive\$" checksums.txt | sha256sum -c -)
        tar -xzf "$ROOT/target/tools/$archive" -C "$ROOT/target/tools" nfpm
    fi
fi

export VERSION
sed -e "s|\${STAGE}|$STAGE|g" -e "s|\${PKGDIR}|$PKGDIR|g" -e "s|\${VERSION}|$VERSION|g" \
    "$PKGDIR/nfpm.yaml" >"$OUT/nfpm.yaml"
for packager in deb rpm; do
    echo "==> $packager"
    "$nfpm" package --config "$OUT/nfpm.yaml" --packager "$packager" --target "$OUT/"
done
rm -rf "$STAGE" "$OUT/nfpm.yaml"
(cd "$OUT" && sha256sum ./*.deb ./*.rpm ./*.tar.gz | sed 's| \./| |')
