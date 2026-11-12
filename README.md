# TimeReel

Private, encrypted screenshot timeline for Linux: scroll back through your work day to register your time.

This repository is built by [PsiForge](https://github.com/PsiForge/psiforge) ψ: AI agents do the work, humans approve at the gates.

## About

TimeReel takes a screenshot of your screens every few minutes (default 5), encrypts it at once with your public key and stores it on your own disk. Nothing leaves your machine and nothing unencrypted is written to disk. The viewer (`timereel`) is locked with a password; it lets you scroll back through a day, jump to a time and search by app, so you can fill in your time sheet later.

Three programs make up TimeReel: `timereel` (the viewer and set-up), `timereel-agent` (the capture service, a systemd user service `timereel-agent.service`) and `timereel-tray` (the panel icon). Captures are skipped while the screen is locked, while recording is paused and for apps you exclude. Defaults: WebP images, 30 days of retention.

Design: [docs/architecture.md](docs/architecture.md). Decisions: [docs/adr/](docs/adr/).

## Requirements

- Ubuntu or Debian with systemd and a graphical session (X11 or Wayland).
- On plain GNOME, the AppIndicator extension for the tray icon (the `gnome-shell-extension-appindicator` package; Ubuntu enables it by default). KDE needs nothing extra.
- For Wayland: `xdg-desktop-portal` with a ScreenCast backend (GNOME and KDE provide one).
- At run time: `libxkbcommon-x11-0` and an OpenGL driver for the viewer.

## Install

Build the package (see [Testing](#testing) for the build requirements) or take the `.deb` from the CI package job, then install it:

```
cargo build --release --workspace && cargo deb -p timereel --no-build
sudo apt install ./target/debian/timereel_*.deb
```

The package installs the three programs in `/usr/bin`, the user unit `/usr/lib/systemd/user/timereel-agent.service`, the tray autostart entry, the app menu entry, the icon and the GNOME extension. Installing starts nothing and has no maintainer scripts; the first set-up starts the service.

## First set-up

1. Start `timereel` (from the app menu or a terminal). With no key file it opens the set-up.
2. Choose a password and type it twice. Key creation takes a moment.
3. The **recovery key** is shown once. Write it down or put it in a password manager, tick the box and continue.
4. TimeReel saves the key file, writes the public key to `config.toml` and runs `systemctl --user enable --now timereel-agent.service`. Capture starts; the tray icon shows the state (green disc: recording, grey bars: paused, red: error or permission missing).

Check the service with `systemctl --user status timereel-agent.service`. Logs go to the journal (`journalctl --user -u timereel-agent.service`); set `TIMEREEL_LOG=debug` for more.

## Recovery key

The recovery key is the only way in if you forget your password. It is shown once, has no copy button and is never stored in readable form. Keep it somewhere safe and separate from this computer: on paper in a safe place, or in a password manager.

**Without the password or the recovery key the data cannot be recovered.** There is no reset, no back door and no way for anyone, including the authors, to open your captures. If you lose both, the only option is `timereel purge-data` and starting over.

## Wayland permission and restore token

On Wayland the agent captures through the ScreenCast portal (ADR 0008).

- The first time, your desktop shows a permission dialog. Allow screen sharing; the choice is remembered through a **restore token** stored in `~/.local/state/timereel/portal-restore-token`, so the dialog does not return on every capture.
- Until you allow it, the state is `needs_permission`. If you decline, the agent does not ask again; use **Grant capture permission** in the tray menu to be asked again. If the token is rejected (for example after a desktop reset), the dialog appears once more.
- The desktop may flash a brief screen-sharing indicator at each capture. This is expected.
- To know which app is active on GNOME Wayland, enable the TimeReel GNOME extension (installed by the package), then log out and in again if the shell does not see it:

  ```
  gnome-extensions enable timereel-focus@timereel.org
  ```

  Without it the app is `unknown` and, by default, those captures are skipped (the state turns to `error` with a hint). Alternatively set `capture_when_app_unknown = true` in `config.toml`.

On X11 no permission is needed.

## Data locations

| What | Where |
|---|---|
| Data: `keyring.json`, `captures/` (encrypted) | `~/.local/share/timereel` |
| Config: `config.toml` (interval, quality, retention, excluded apps) | `~/.config/timereel` |
| State: `portal-restore-token` | `~/.local/state/timereel` |

The `XDG_DATA_HOME`, `XDG_CONFIG_HOME` and `XDG_STATE_HOME` variables move them; `TIMEREEL_DATA_DIR`, `TIMEREEL_CONFIG_DIR` and `TIMEREEL_STATE_DIR` override them. Folders are mode 0700 and files 0600.

## Security notes

- **Not encrypted:** capture times (in the file names and file times) and file sizes. Anyone with the disk can see when you worked, but not what was on screen. This is needed so retention and the time index work without the password (ADR 0006).
- **Swap:** decrypted images live in RAM while the viewer is open and may be swapped out. Use full-disk encryption or encrypted swap.
- **Full-disk encryption** is recommended in any case, for the config, the logs and the metadata above.
- **Old key-file backups:** copies of `keyring.json` in backups still open with the password that was valid when they were made. After changing your password, delete old backups of `keyring.json`.
- Not protected against: malware running as your user while the session is unlocked, and root.
- The viewer disables core dumps and writes no thumbnail cache or other data derived from decrypted images.

## Testing

Requires the Rust toolchain from `rust-toolchain.toml` (install via [rustup](https://rustup.rs)) and, on Ubuntu:
`sudo apt-get install pkg-config libclang-dev libpipewire-0.3-dev libxkbcommon-dev xvfb dbus`.

```
cargo build --workspace                                    # all crates and the three binaries
cargo test --workspace
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo deny check                                           # cargo install cargo-deny --locked
cargo run --release -p timereel-bench                      # benchmark; thresholds in ADR 0013
```

`timereel-bench` is still a placeholder crate; the benchmark (decrypt and decode times against the limits in [ADR 0013](docs/adr/0013-benchmark-thresholds.md)) arrives with its own ticket.

CI (`.github/workflows/ci.yml`) runs the format, clippy, test and cargo-deny steps on every push and pull request. How to take part: tickets are issues with a `psi:<state>` label; approvers steer with comments `/approve`, `/reject <reason>`, `/answer <text>`, `/escalate`, `/stop`; nothing reaches `main` without a human merging the pull request.

## Uninstall and data deletion

Delete your data first, then remove the programs:

```
timereel purge-data
sudo apt remove timereel
```

`timereel purge-data` lists the data, config and state folders, asks you to type `DELETE`, runs `systemctl --user disable --now timereel-agent.service` and deletes only those folders. Run it before `apt remove`, because the service has to be known to systemd. `apt remove timereel` removes only the programs and leaves your data. Delete old backups of the data folder yourself.

## Component notes (development)

#### The agent

`timereel-agent` runs as a systemd user service (`packaging/timereel-agent.service`) and serves `org.timereel.Agent1` on the session bus. For now its real screen and lock sources report an unsupported session (X11 and Wayland capture come in later tickets), so it only shows its state. Try it:

```
cargo build -p timereel-agent
dbus-run-session -- bash -c 'target/debug/timereel-agent & sleep 1;
  busctl --user get-property org.timereel.Agent1 /org/timereel/Agent1 org.timereel.Agent1 State'
```

The D-Bus integration tests (`apps/timereel-agent/tests/dbus.rs`) start their own private `dbus-daemon`, so plain `cargo test` works; CI runs them under `dbus-run-session`. Set `TIMEREEL_LOG=debug` for more log lines (the agent logs to the journal, or to stderr if there is no journal). To check the unit file: `systemd-analyze --user verify packaging/timereel-agent.service` (needs `timereel-agent` installed at `/usr/bin`).

#### The tray

`timereel-tray` shows the agent state in the panel (a StatusNotifierItem through `ksni`, ADR 0003): a green disc while recording, grey bars while paused, red for an error, for `needs_permission` and when the agent is not running. The tooltip shows the reason. The menu has Pause/Resume, Open TimeReel (activates `org.timereel.Viewer1`, or starts `timereel`), Grant capture permission (only when permission is missing) and Quit (asks the viewer to quit and ends the tray; the agent keeps running). It follows the agent through `PropertiesChanged` and `NameOwnerChanged`. Autostart: install `packaging/timereel-tray.desktop` to `/etc/xdg/autostart/`.

On plain GNOME the icon needs the AppIndicator extension (Ubuntu enables it by default); KDE works as is. Try it with the agent running: `cargo run -p timereel-tray`.

The integration tests in `apps/timereel-tray/tests/dbus.rs` start a private `dbus-daemon` with a fake agent and viewer, and click the menu of the real `timereel-tray` process through `com.canonical.dbusmenu`.

The X11 capture tests (`crates/timereel-capture/tests/x11.rs`) start their own private `Xvfb` servers (2560x1080 with two RandR monitors for the capture test), so they need the `xvfb` package but not `xvfb-run`.

#### The viewer and first-run set-up

`timereel` (egui/eframe with the glow backend, no persistence, ADR 0002) claims `org.timereel.Viewer1` on the session bus, so only one runs: a second `timereel` calls `Activate` on the first and exits. It turns off core dumps with `prctl(PR_SET_DUMPABLE, 0)`. With no key file (`~/.local/share/timereel/keyring.json`) it opens the set-up: password and confirmation, key creation on a worker thread, the recovery key shown once (no copy button, a tick box is required), then it saves the key file, writes `public_key` to `config.toml` and runs `systemctl --user enable --now timereel-agent.service`. If a key file exists it shows a placeholder until the lock screen arrives. Running it needs `libxkbcommon-x11-0` and an OpenGL driver at run time.

Tests: the set-up pages use `egui_kittest` (headless); the single-instance test starts its own private `dbus-daemon`; the set-up flow tests point `TIMEREEL_*_DIR`, `XDG_CACHE_HOME` and `TMPDIR` at a temp tree and use a fake `systemctl`.

#### The .deb and `timereel purge-data`

```
cargo build --release --workspace && cargo deb -p timereel --no-build   # cargo install cargo-deb --locked
packaging/check-deb.sh target/debian/timereel_*.deb                     # lintian, apt install, file and unit checks (needs sudo)
```

The package (ADR 0012) installs the three programs in `/usr/bin`, the user unit `/usr/lib/systemd/user/timereel-agent.service`, the tray autostart entry, the app menu entry, the icon and the GNOME extension. It has no maintainer scripts: installing starts nothing; the viewer's first-run set-up enables the service. `apt remove timereel` removes only the programs.

`timereel purge-data` lists the data, config and state folders (`~/.local/share/timereel`, `~/.config/timereel`, `~/.local/state/timereel`, or the `TIMEREEL_*_DIR` overrides), asks you to type `DELETE`, runs `systemctl --user disable --now timereel-agent.service` and deletes only those folders. It refuses any folder that is not an absolute path ending in `timereel`, and deletes nothing if `systemctl` fails. Run it before `apt remove` (the service has to be known to systemd).
