# 0007. 0007 Capture on X11 with x11rb and RandR

## Context
On X11 all monitors must be captured and stitched following the layout, with low memory, and the active app and title must be read.

## Options
- **`xcap` / `screenshots` crates:** convenient, but they allocate full frames per monitor, and their Wayland path does not fit our portal design.
- **Spawning `import` or `scrot`:** extra processes, and they may write temp files. Rejected.
- **`x11rb` directly:** pure Rust, maintained. It gives RandR `GetMonitors`, `GetImage` on the root window in bands, and EWMH properties for the active window.

## Decision
x11rb. Each monitor rectangle is grabbed in bands and scaled into the output canvas. The active window comes from `_NET_ACTIVE_WINDOW` + `WM_CLASS` + `_NET_WM_NAME`.

## Consequences
Peak memory stays small. It can be tested in CI under Xvfb with virtual monitors (`xrandr --setmonitor`). Window managers without EWMH report 'no window'.
