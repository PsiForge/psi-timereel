# 0001. 0001 Use Rust in one Cargo workspace for all programs

## Context
The agent must be small: under 30 MB memory, under 1 % CPU, and asleep between ticks. The viewer must decode and show images smoothly. Both need the same crypto and storage code, and that code must be tested in CI.

## Options
- **Rust:** low memory use and no garbage collector. It has mature crates for crypto (age, RustCrypto), X11 (x11rb), D-Bus (zbus), portals (ashpd) and PipeWire.
- **Go:** also small, but GUI and PipeWire support are weaker.
- **Python:** quick to write, but too heavy for an always-on agent, and the image path is slow.
- **C++/Qt:** good GUI, but the risk of memory-safety bugs in parsing and crypto code is higher.

## Decision
Rust (stable, pinned in `rust-toolchain.toml`). There is one workspace with shared library crates and three binaries: agent, tray and viewer.

## Consequences
The agent, tray and viewer share one tested implementation of the formats. CI needs a few system packages (libpipewire, libclang for bindgen). The developer agents work in Rust only, apart from a small GNOME extension in JavaScript.
