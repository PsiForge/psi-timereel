# timereel

Private, encrypted screenshot timeline for Linux: scroll back through your work day to register your time.

This repository is built by [PsiForge](https://github.com/PsiForge/psiforge) ψ: AI agents do the work, humans approve at the gates.

## How to take part

- Tickets are issues with a `psi:<state>` label.
- Approvers (@AndriMar) steer with comments on a line of their own: `/approve`, `/reject <reason>`, `/answer <text>`, `/escalate`, `/stop`.
- Nothing reaches `main` without a human merging the pull request.

## Build and test

Requires the Rust toolchain from `rust-toolchain.toml` (install via [rustup](https://rustup.rs)) and, on Ubuntu:
`sudo apt-get install pkg-config libclang-dev libpipewire-0.3-dev libxkbcommon-dev xvfb dbus`.

```
cargo build --workspace                                    # all crates and the three binaries
cargo run -p timereel                                      # also: timereel-agent, timereel-tray
cargo test --workspace
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo deny check                                           # cargo install cargo-deny --locked
```

CI (`.github/workflows/ci.yml`) runs the format, clippy, test and cargo-deny steps on every push and pull request.

### The agent

`timereel-agent` runs as a systemd user service (`packaging/timereel-agent.service`) and serves `org.timereel.Agent1` on the session bus. For now its real screen and lock sources report an unsupported session (X11 and Wayland capture come in later tickets), so it only shows its state. Try it:

```
cargo build -p timereel-agent
dbus-run-session -- bash -c 'target/debug/timereel-agent & sleep 1;
  busctl --user get-property org.timereel.Agent1 /org/timereel/Agent1 org.timereel.Agent1 State'
```

The D-Bus integration tests (`apps/timereel-agent/tests/dbus.rs`) start their own private `dbus-daemon`, so plain `cargo test` works; CI runs them under `dbus-run-session`. Set `TIMEREEL_LOG=debug` for more log lines (the agent logs to the journal, or to stderr if there is no journal). To check the unit file: `systemd-analyze --user verify packaging/timereel-agent.service` (needs `timereel-agent` installed at `/usr/bin`).

### The tray

`timereel-tray` shows the agent state in the panel (a StatusNotifierItem through `ksni`, ADR 0003): a green disc while recording, grey bars while paused, red for an error, for `needs_permission` and when the agent is not running. The tooltip shows the reason. The menu has Pause/Resume, Open TimeReel (activates `org.timereel.Viewer1`, or starts `timereel`), Grant capture permission (only when permission is missing) and Quit (asks the viewer to quit and ends the tray; the agent keeps running). It follows the agent through `PropertiesChanged` and `NameOwnerChanged`. Autostart: install `packaging/timereel-tray.desktop` to `/etc/xdg/autostart/`.

On plain GNOME the icon needs the AppIndicator extension (Ubuntu enables it by default); KDE works as is. Try it with the agent running: `cargo run -p timereel-tray`.

The integration tests in `apps/timereel-tray/tests/dbus.rs` start a private `dbus-daemon` with a fake agent and viewer, and click the menu of the real `timereel-tray` process through `com.canonical.dbusmenu`.

The X11 capture tests (`crates/timereel-capture/tests/x11.rs`) start their own private `Xvfb` servers (2560x1080 with two RandR monitors for the capture test), so they need the `xvfb` package but not `xvfb-run`.

### The viewer and first-run set-up

`timereel` (egui/eframe with the glow backend, no persistence, ADR 0002) claims `org.timereel.Viewer1` on the session bus, so only one runs: a second `timereel` calls `Activate` on the first and exits. It turns off core dumps with `prctl(PR_SET_DUMPABLE, 0)`. With no key file (`~/.local/share/timereel/keyring.json`) it opens the set-up: password and confirmation, key creation on a worker thread, the recovery key shown once (no copy button, a tick box is required), then it saves the key file, writes `public_key` to `config.toml` and runs `systemctl --user enable --now timereel-agent.service`. If a key file exists it shows a placeholder until the lock screen arrives. Running it needs `libxkbcommon-x11-0` and an OpenGL driver at run time.

Tests: the set-up pages use `egui_kittest` (headless); the single-instance test starts its own private `dbus-daemon`; the set-up flow tests point `TIMEREEL_*_DIR`, `XDG_CACHE_HOME` and `TMPDIR` at a temp tree and use a fake `systemctl`.

### The .deb and `timereel purge-data`

```
cargo build --release --workspace && cargo deb -p timereel --no-build   # cargo install cargo-deb --locked
packaging/check-deb.sh target/debian/timereel_*.deb                     # lintian, apt install, file and unit checks (needs sudo)
```

The package (ADR 0012) installs the three programs in `/usr/bin`, the user unit `/usr/lib/systemd/user/timereel-agent.service`, the tray autostart entry, the app menu entry, the icon and the GNOME extension. It has no maintainer scripts: installing starts nothing; the viewer's first-run set-up enables the service. `apt remove timereel` removes only the programs.

`timereel purge-data` lists the data, config and state folders (`~/.local/share/timereel`, `~/.config/timereel`, `~/.local/state/timereel`, or the `TIMEREEL_*_DIR` overrides), asks you to type `DELETE`, runs `systemctl --user disable --now timereel-agent.service` and deletes only those folders. It refuses any folder that is not an absolute path ending in `timereel`, and deletes nothing if `systemctl` fails. Run it before `apt remove` (the service has to be known to systemd).
