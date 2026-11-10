# 0003. 0003 Run the tray as a separate small process using ksni

## Context
The tray must start at login and show the agent state. It must allow pause without unlocking, and Quit must not stop the agent. winit (used by eframe) cannot hide a window on Wayland, so a combined 'hidden viewer plus tray' process does not work well there.

## Options
- **Tray inside the viewer** (tray-icon crate + GTK main loop): this clashes with the winit event loop, and an idle GPU context would sit in memory all day.
- **Tray inside the agent:** the agent would need GUI dependencies and would use more memory.
- **Separate `timereel-tray` process using ksni:** ksni is pure Rust (StatusNotifierItem over zbus) and needs no GTK.

## Decision
A separate `timereel-tray` process, started by XDG autostart. It talks to the agent over D-Bus and starts or activates the viewer (`org.timereel.Viewer1`).

## Consequences
Closing the viewer ends its process and frees all decrypted data. StatusNotifierItem needs the AppIndicator extension on plain GNOME; Ubuntu enables it by default. The README documents this.
