#!/bin/sh
# Nach der Installation des .deb/.rpm-Pakets: Hinweise zur Einrichtung.
greeter=greeter
for u in greeter _greetd greetd; do
    if getent passwd "$u" >/dev/null 2>&1; then
        greeter=$u
        break
    fi
done
cat <<EOF
==> TUILNUX ist installiert.
    Ersteinrichtung (Sprache, Tastatur, Bildschirm):  tuilnux-setup
    Ausprobieren (in einer TTY oder einem Terminal):  tuilnux
    Shell-Integration:
        echo '[ -f /usr/share/tuilnux/bashrc.sh ] && . /usr/share/tuilnux/bashrc.sh' >> ~/.bashrc
    Anmeldung beim Booten (greetd + tlogin):
        sed 's/^user = .*/user = "$greeter"/' /usr/share/tuilnux/greetd/config.toml | sudo tee /etc/greetd/config.toml
        sudo systemctl enable greetd.service   (vorher den alten Anmeldemanager deaktivieren)
    Backend wählen: /etc/tuilnux/session.conf (auto | labwc | cage | tty)
    Anleitung: /usr/share/doc/tuilnux/ANLEITUNG.md
EOF
exit 0
