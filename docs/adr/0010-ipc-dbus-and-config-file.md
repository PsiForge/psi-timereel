# 0010. 0010 Use D-Bus for control and state, and one locked TOML file for settings

## Context
The viewer and tray must pause, resume and reload the agent, and must see its state within seconds. Settings must survive restarts, and the paused state too.

## Options
- **File watch only (inotify):** simple, but there is no way to read the state back, and the tray would have to poll.
- **Unix socket with our own protocol:** more code to write and test.
- **D-Bus session bus with zbus:** standard on Linux desktops, with properties and change signals built in. It is pure Rust and testable with `dbus-run-session`.

## Decision
- The agent serves `org.timereel.Agent1` (State, Reason, Pause, Resume, ReloadConfig, RunRetention, RetryPermission).
- Settings, including `paused`, live in `config.toml`. All writers use a `flock`-guarded read-modify-write with an atomic replace. The viewer writes the settings; the agent writes `paused`.
- The agent re-reads the config at every tick as a fallback.

## Consequences
The tray follows the state through `PropertiesChanged`. If the agent is not running, the viewer can still save settings, and they apply at the next start. The D-Bus API never carries content.
