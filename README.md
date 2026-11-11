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
