//! TimeReel capture sources (architecture sections 2.1 and 5).
//!
//! The agent talks to the outside world through three small traits:
//!
//! * [`ScreenSource`]: grabs one frame per monitor, with its position;
//! * [`ActiveWindowSource`]: says which app and window has the focus;
//! * [`LockSource`]: says whether the screen is locked.
//!
//! Real implementations (X11, the Wayland portal, the GNOME extension,
//! logind) come in later tickets. The [`fake`] module has scriptable fakes so
//! the agent's skip and error logic can be tested without a display.
//!
//! Error values and their `Display` text never contain window titles or app
//! names, so they are safe to log.

pub mod fake;
pub mod lock;

use timereel_image::Frame;

/// The active window as reported by an [`ActiveWindowSource`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActiveWindow {
    /// A window has the focus and its app and title could be read.
    Known {
        /// App name, for example the X11 `WM_CLASS` class.
        app: String,
        /// Window title. May be empty.
        title: String,
    },
    /// No window has the focus at all (for example the bare desktop).
    NoWindow,
    /// The source could not find out (for example no extension on Wayland).
    Unknown,
}

/// Why a screen could not be captured. There is no free text so nothing
/// sensitive can end up in a log.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum CaptureError {
    /// No usable display or session was found.
    #[error("no display")]
    NoDisplay,
    /// The user (or the desktop) did not allow screen capture.
    #[error("no permission")]
    NoPermission,
    /// The screen capture portal failed.
    #[error("portal error")]
    Portal,
}

/// Grabs the screen.
pub trait ScreenSource {
    /// Captures every monitor once. Each [`Frame`] holds the pixels of one
    /// monitor and its position in the combined layout.
    fn capture(&self) -> Result<Vec<Frame>, CaptureError>;
}

/// Reads the focused window.
pub trait ActiveWindowSource {
    /// Returns the focused window. A source that cannot tell returns
    /// [`ActiveWindow::Unknown`]; it has no error case on purpose.
    fn active_window(&self) -> ActiveWindow;
}

/// Reads the lock state of the screen.
pub trait LockSource {
    /// Whether the screen is locked. A source that cannot tell returns
    /// `false`: a wrong guess at worst captures a lock screen (section 5).
    fn is_locked(&self) -> bool;
}

// Shared sources: a test (or the agent) can keep an `Arc` to a source to look
// at it while the runner owns another handle.
impl<T: ScreenSource + ?Sized> ScreenSource for std::sync::Arc<T> {
    fn capture(&self) -> Result<Vec<Frame>, CaptureError> {
        (**self).capture()
    }
}

impl<T: ActiveWindowSource + ?Sized> ActiveWindowSource for std::sync::Arc<T> {
    fn active_window(&self) -> ActiveWindow {
        (**self).active_window()
    }
}

impl<T: LockSource + ?Sized> LockSource for std::sync::Arc<T> {
    fn is_locked(&self) -> bool {
        (**self).is_locked()
    }
}
