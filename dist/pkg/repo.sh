#!/usr/bin/env bash
# Erzeugt die Paketquellen für GitHub Pages (Branch gh-pages):
#   apt/  – .deb + Packages/Release  (Debian, Ubuntu, Mint, …)
#   rpm/  – .rpm + repodata          (Fedora, openSUSE, …)
#
#   bash dist/pkg/repo.sh [PAKETE] [ZIEL]
#     PAKETE  Verzeichnis mit .deb/.rpm (Standard: target/packages)
#     ZIEL    Ausgabeverzeichnis       (Standard: target/pages)
#
# Benötigt apt-ftparchive und createrepo_c – fehlen sie, werden
# Container (podman oder docker) mit Debian bzw. Fedora verwendet.
set -euo pipefail

ROOT=$(cd "$(dirname "$0")/../.." && pwd)
PKGS=$(realpath "${1:-$ROOT/target/packages}")
OUT=$(realpath -m "${2:-$ROOT/target/pages}")
URL=${TUILNUX_PAGES_URL:-https://dnsx64.github.io/TUILNUX}

shopt -s nullglob
debs=("$PKGS"/*.deb)
rpms=("$PKGS"/*.rpm)
((${#debs[@]} && ${#rpms[@]})) || { echo "Keine .deb/.rpm in $PKGS – zuerst dist/pkg/build.sh ausführen." >&2; exit 1; }

engine=''
for e in podman docker; do command -v "$e" >/dev/null && { engine=$e; break; }; done

run_in() { # Abbild Verzeichnis Befehl
    if [[ -z $engine ]]; then
        echo "Weder Werkzeug noch podman/docker vorhanden." >&2
        exit 1
    fi
    "$engine" run --rm -v "$2:/w:z" "$1" bash -c "$3"
}

mkdir -p "$OUT/apt" "$OUT/rpm"
rm -f "$OUT"/apt/*.deb "$OUT"/rpm/*.rpm
cp "${debs[@]}" "$OUT/apt/"
cp "${rpms[@]}" "$OUT/rpm/"

APT_CMD='cd /w && rm -f Release && apt-ftparchive packages . > Packages && gzip -kf Packages &&
  apt-ftparchive -o APT::FTPArchive::Release::Origin=TUILNUX -o APT::FTPArchive::Release::Label=TUILNUX \
    -o APT::FTPArchive::Release::Suite=stable release . > /tmp/Release && mv /tmp/Release Release'
if command -v apt-ftparchive >/dev/null; then
    (APT_DIR=$OUT/apt; bash -c "${APT_CMD//\/w/$APT_DIR}")
else
    run_in debian:stable "$OUT/apt" "export DEBIAN_FRONTEND=noninteractive; apt-get update -qq &&
        apt-get install -y -qq apt-utils >/dev/null 2>&1 && $APT_CMD"
fi

# gzip statt zstd: auch ältere dnf/zypper-Versionen lesen die Metadaten.
RPM_CMD='rm -rf /w/repodata && createrepo_c -q --general-compress-type gz /w'
if command -v createrepo_c >/dev/null; then
    bash -c "${RPM_CMD//\/w/$OUT/rpm}"
else
    run_in fedora:latest "$OUT/rpm" "dnf install -y -q createrepo_c >/dev/null 2>&1 && $RPM_CMD"
fi

cat >"$OUT/rpm/tuilnux.repo" <<EOF
[tuilnux]
name=TUILNUX
baseurl=$URL/rpm
enabled=1
gpgcheck=0
repo_gpgcheck=0
EOF

touch "$OUT/.nojekyll"
cat >"$OUT/index.html" <<EOF
<!doctype html>
<html lang="de"><head><meta charset="utf-8"><title>TUILNUX – Paketquellen</title>
<style>body{font-family:monospace;max-width:52em;margin:2em auto;padding:0 1em;background:#111;color:#ddd}
a{color:#6cf}pre{background:#222;padding:.8em;overflow-x:auto}</style></head><body>
<h1>TUILNUX – Paketquellen</h1>
<p>Projekt: <a href="https://github.com/DNSx64/TUILNUX">github.com/DNSx64/TUILNUX</a></p>
<h2>Debian / Ubuntu / Mint</h2>
<pre>echo 'deb [trusted=yes] $URL/apt ./' | sudo tee /etc/apt/sources.list.d/tuilnux.list
sudo apt update &amp;&amp; sudo apt install tuilnux</pre>
<h2>Fedora</h2>
<pre>sudo curl -fsSLo /etc/yum.repos.d/tuilnux.repo $URL/rpm/tuilnux.repo
sudo dnf install tuilnux</pre>
<h2>openSUSE</h2>
<pre>sudo zypper ar -G -f $URL/rpm tuilnux
sudo zypper in tuilnux</pre>
<h2>Arch Linux</h2>
<p>Siehe <a href="https://github.com/DNSx64/TUILNUX#installation">README</a> (pacman-Repo bzw. AUR-PKGBUILD).</p>
<p>Danach einmal <code>tuilnux-setup</code> starten (Sprache, Tastatur, Bildschirm).</p>
</body></html>
EOF

echo "Paketquellen in $OUT:"
find "$OUT" -maxdepth 2 -type f | sort
