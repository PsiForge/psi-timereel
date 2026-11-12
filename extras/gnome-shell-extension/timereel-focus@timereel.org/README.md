# TimeReel Focus (GNOME Shell extension)

Lets `timereel-agent` read the focused app on GNOME Wayland (ADR 0009). It
exports `org.timereel.Focus1.GetFocus() -> (ok, app_name, wm_class, title)` on
the session bus. For GNOME 45 and later.

- `ok = false`: the extension could not tell; TimeReel treats the app as unknown.
- `ok = true` and three empty strings: no window has the focus.
- Otherwise the focused window: the app name from `Shell.WindowTracker`, the
  window class and the title.

The extension never logs anything, so titles cannot reach the journal.

## Try it by hand

```sh
mkdir -p ~/.local/share/gnome-shell/extensions
cp -r timereel-focus@timereel.org ~/.local/share/gnome-shell/extensions/
# log out and in once (Wayland), then:
gnome-extensions enable timereel-focus@timereel.org
gdbus call --session --dest org.timereel.Focus1 \
  --object-path /org/timereel/Focus1 --method org.timereel.Focus1.GetFocus
```

The `.deb` installs it system-wide under `/usr/share/gnome-shell/extensions/`.
