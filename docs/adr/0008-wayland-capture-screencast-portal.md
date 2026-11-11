# 0008. 0008 Capture on Wayland with the ScreenCast portal and PipeWire, using restore tokens

## Context
On Wayland, capture must go through xdg-desktop-portal. Permission must be granted once and kept, and nothing unencrypted may be written to disk. Story #10 asks to check silent screenshots.

## Options
- **Screenshot portal:** it returns a URI to a PNG file that the portal has written to disk. This breaks 'no plaintext on disk'. Silent, non-interactive use is also not reliable for unsandboxed apps. Rejected.
- **wlr-screencopy protocol:** works on wlroots compositors only, not GNOME, and bypasses the portal.
- **A ScreenCast stream kept open all the time:** the compositor keeps producing frames (CPU use) and the sharing indicator stays on.
- **A short ScreenCast session per capture:** with `persist_mode = 2` and a restore token, using `ashpd` (portal) and `pipewire` (frames, SHM buffers).

## Decision
A short ScreenCast session per capture: open, grab one frame per monitor stream, close. The single-use restore token is saved after every start in `$XDG_STATE_HOME/timereel/portal-restore-token` (mode 0600). If the token is missing or rejected, the agent prompts once and shows `needs_permission`. It does not prompt again until the user asks from the tray. Portal version 4 or later is required; older versions give a clear error.

## Consequences
No files are written by the portal, and CPU use stays near zero between ticks. A short screen-sharing indicator may appear at each capture. libpipewire is a runtime dependency. Real behaviour can only be checked by the owner on GNOME Wayland at the merge gate; CI tests the token and permission logic with a fake portal.
