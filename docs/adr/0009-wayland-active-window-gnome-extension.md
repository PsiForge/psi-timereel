# 0009. 0009 Read the active window on GNOME Wayland through a small bundled GNOME Shell extension

## Context
Wayland has no standard way to read the focused app and title. GNOME disabled `Shell.Eval`. Excluded apps depend on this information, and the safe default skips captures when the app is unknown.

## Options
- **XWayland properties:** they only see X11 apps, so they are wrong for native Wayland apps.
- **Third-party extensions (for example 'Window Calls'):** an external dependency we do not control.
- **Own minimal extension:** it exposes `org.timereel.Focus1.GetFocus()` on the session bus using `global.display.focus_window` and `Shell.WindowTracker`.
- **wlr-foreign-toplevel:** works for wlroots compositors only. Deferred, because the owner uses Ubuntu (GNOME).

## Decision
Ship our own extension, about 50 lines of JavaScript (GNOME 45+ ES modules), installed system-wide by the `.deb`. The user enables it once. Without it, the app is reported as `unknown`.

## Consequences
With the default settings, GNOME Wayland without the extension skips all captures. The agent shows this as an error with a clear reason, so it is not silent. Other compositors report unknown until a later ticket adds them. The extension can only be tested on a real GNOME session at the merge gate; CI checks the JavaScript syntax.
