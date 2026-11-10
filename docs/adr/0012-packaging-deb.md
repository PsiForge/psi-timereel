# 0012. 0012 Package as a .deb with cargo-deb, user-level service, and an explicit purge command

## Context
TimeReel must install on Ubuntu/Debian, run as the user and never as root, and uninstall cleanly. Deleting data must be a separate, explicit step. Signed installers are out of scope.

## Options
- **Shell install script into `~/.local`:** no root needed, but upgrades and removal are manual.
- **Flatpak:** sandboxing makes X11 capture, the extension and the user service awkward.
- **.deb built with `cargo-deb`:** installs the binaries, the systemd *user* unit, the autostart and desktop entries, icons and the GNOME extension. There are no maintainer scripts that start services.

## Decision
A .deb built with cargo-deb. The viewer's set-up enables the service for the logged-in user (`systemctl --user enable --now timereel-agent.service`). `timereel purge-data` stops and disables the user service and deletes the data, config and state folders after the user types DELETE. `apt remove` removes only the programs.

## Consequences
Installing needs sudo once; runtime never does. The README documents the order: purge-data (optional), then apt remove. CI builds the package, runs lintian and installs it.
