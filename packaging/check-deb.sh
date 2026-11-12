#!/bin/bash
# Checks the TimeReel .deb: lintian, install with apt, the installed files and
# the systemd user unit. Used by the "package" CI job; runs on Debian/Ubuntu and
# needs sudo, lintian and systemd-analyze.
#
#   packaging/check-deb.sh target/debian/timereel_0.1.0-1_amd64.deb
set -euo pipefail

deb=${1:?usage: check-deb.sh <file.deb>}
deb=$(realpath "$deb")
fail() { echo "FAIL: $*" >&2; exit 1; }

echo "== lintian (errors fail the job)"
lintian --fail-on error "$deb"

echo "== package has no maintainer scripts"
ctl=$(mktemp -d)
dpkg-deb -e "$deb" "$ctl"
for script in preinst postinst prerm postrm triggers; do
    [ ! -e "$ctl/$script" ] || fail "the package has a $script script"
done
dpkg-deb -f "$deb" Depends | grep -q 'libpipewire-0.3-0' || fail "Depends has no libpipewire-0.3-0"

echo "== install with apt"
sudo apt-get install -y "$deb"

echo "== listed files exist"
files=(
    /usr/bin/timereel
    /usr/bin/timereel-agent
    /usr/bin/timereel-tray
    /usr/lib/systemd/user/timereel-agent.service
    /etc/xdg/autostart/timereel-tray.desktop
    /usr/share/applications/timereel.desktop
    /usr/share/icons/hicolor/scalable/apps/timereel.svg
    "/usr/share/gnome-shell/extensions/timereel-focus@timereel.org/extension.js"
    "/usr/share/gnome-shell/extensions/timereel-focus@timereel.org/metadata.json"
)
for f in "${files[@]}"; do
    [ -f "$f" ] || fail "missing $f"
done
for b in timereel timereel-agent timereel-tray; do
    [ -x "/usr/bin/$b" ] || fail "/usr/bin/$b is not executable"
done
/usr/bin/timereel --version

echo "== the unit is a user unit"
unit=/usr/lib/systemd/user/timereel-agent.service
if grep -Eq '^[[:space:]]*User=' "$unit"; then fail "$unit has a User= line"; fi
grep -q '^ExecStart=/usr/bin/timereel-agent$' "$unit" || fail "unexpected ExecStart in $unit"
grep -q '^WantedBy=graphical-session.target$' "$unit" || fail "$unit has no [Install] WantedBy"
for system_dir in /usr/lib/systemd/system /lib/systemd/system /etc/systemd/system; do
    [ -z "$(ls "$system_dir" 2>/dev/null | grep -i timereel || true)" ] || fail "a TimeReel unit is installed as a system unit in $system_dir"
done
systemd-analyze --user verify "$unit"

echo "== installing started and enabled nothing"
if pgrep -x -a 'timereel.*' ; then fail "a TimeReel process is running after install"; fi
leftovers=$(find /etc/systemd /etc/xdg/systemd /usr/lib/systemd/user /usr/lib/systemd/system \
    -type l -iname '*timereel*' 2>/dev/null || true)
[ -z "$leftovers" ] || fail "install enabled a unit: $leftovers"

echo "== apt remove removes the programs"
sudo apt-get remove -y timereel
for b in timereel timereel-agent timereel-tray; do
    [ ! -e "/usr/bin/$b" ] || fail "/usr/bin/$b is still there after remove"
done
[ ! -e "$unit" ] || fail "$unit is still there after remove"

echo "OK"
