//! The real capture sources. Until the X11, Wayland and logind sources exist
//! (later tickets) they report an unsupported session.

use timereel_capture::{ActiveWindow, ActiveWindowSource, CaptureError, LockSource, ScreenSource};
use timereel_image::Frame;

/// Screen source for a session type the agent cannot capture yet. Every
/// capture fails with [`CaptureError::NoDisplay`] ("No display found"), and
/// the log says why, once.
pub struct UnsupportedScreen {
    warned: std::sync::atomic::AtomicBool,
}

impl UnsupportedScreen {
    /// A new source.
    pub fn new() -> Self {
        Self {
            warned: std::sync::atomic::AtomicBool::new(false),
        }
    }
}

impl Default for UnsupportedScreen {
    fn default() -> Self {
        Self::new()
    }
}

impl ScreenSource for UnsupportedScreen {
    fn capture(&self) -> Result<Vec<Frame>, CaptureError> {
        if !self.warned.swap(true, std::sync::atomic::Ordering::Relaxed) {
            tracing::warn!("unsupported session: no screen source is available");
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
