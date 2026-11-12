//! The real capture sources. X11 sessions use the X11 sources of
//! `timereel-capture`. Until the Wayland and logind sources exist (later
//! tickets) those sessions report as not supported.

use timereel_capture::{
    ActiveWindow, ActiveWindowSource, CaptureError, LockSource, ScreenSource, Session, X11Screen,
    X11Window,
};
use timereel_image::Frame;

/// The screen and active-window sources for a session type.
///
/// * X11: the real X11 sources, which connect at the first capture and again
///   after a lost connection.
/// * Wayland: not yet supported (every capture is `NoDisplay`, with a log
///   line saying why, once; the app is unknown).
/// * No session: every capture is `NoDisplay`.
pub fn screen_and_window(session: Session) -> (Box<dyn ScreenSource>, Box<dyn ActiveWindowSource>) {
    match session {
        Session::X11 => (Box::new(X11Screen::new()), Box::new(X11Window::new())),
        Session::Wayland => (
            Box::new(UnsupportedScreen::new(
                "Wayland capture is not yet supported",
            )),
            Box::new(UnsupportedWindow),
        ),
        Session::None => (
            Box::new(UnsupportedScreen::new("no graphical session found")),
            Box::new(UnsupportedWindow),
        ),
    }
}

/// Screen source for a session the agent cannot capture. Every capture fails
/// with [`CaptureError::NoDisplay`] ("No display found"), and the log says
/// why, once.
pub struct UnsupportedScreen {
    why: &'static str,
    warned: std::sync::atomic::AtomicBool,
}

impl UnsupportedScreen {
    /// A new source. `why` is a fixed text for the log (no user content).
    pub fn new(why: &'static str) -> Self {
        Self {
            why,
            warned: std::sync::atomic::AtomicBool::new(false),
        }
    }
}

impl ScreenSource for UnsupportedScreen {
    fn capture(&self) -> Result<Vec<Frame>, CaptureError> {
        if !self.warned.swap(true, std::sync::atomic::Ordering::Relaxed) {
            tracing::warn!(reason = self.why, "no screen source is available");
        }
        Err(CaptureError::NoDisplay)
    }
}

/// Lock source for a session type with no lock detection yet: the screen is
/// treated as unlocked (architecture section 5), with one warning in the log.
pub struct UnsupportedLock {
    warned: std::sync::atomic::AtomicBool,
}

impl UnsupportedLock {
    /// A new source.
    pub fn new() -> Self {
        Self {
            warned: std::sync::atomic::AtomicBool::new(false),
        }
    }
}

impl Default for UnsupportedLock {
    fn default() -> Self {
        Self::new()
    }
}

impl LockSource for UnsupportedLock {
    fn is_locked(&self) -> bool {
        if !self.warned.swap(true, std::sync::atomic::Ordering::Relaxed) {
            tracing::warn!("unsupported session: no lock source is available");
        }
        false
    }
}

/// Active-window source for a session type with no window reading yet: the
/// app is unknown, so with the safe default nothing is captured.
#[derive(Debug, Default)]
pub struct UnsupportedWindow;

impl ActiveWindowSource for UnsupportedWindow {
    fn active_window(&self) -> ActiveWindow {
        ActiveWindow::Unknown
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wayland_and_no_session_are_not_captured() {
        for session in [Session::Wayland, Session::None] {
            let (screen, window) = screen_and_window(session);
            assert_eq!(screen.capture().err(), Some(CaptureError::NoDisplay));
            assert_eq!(window.active_window(), ActiveWindow::Unknown);
        }
    }

    #[test]
    fn an_x11_session_without_a_server_is_no_display_and_retries() {
        // The sources of an X11 session connect lazily, so building them never
        // fails; a capture without a reachable server is `NoDisplay`, every
        // time (it reconnects at each call).
        let screen = X11Screen::with_display(Some(":998".to_string()));
        assert_eq!(screen.capture().err(), Some(CaptureError::NoDisplay));
        assert_eq!(screen.capture().err(), Some(CaptureError::NoDisplay));
        let (screen, _window) = screen_and_window(Session::X11);
        let _ = screen; // construction alone must not need a display
    }
}
