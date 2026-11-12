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
