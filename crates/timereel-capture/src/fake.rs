//! Scriptable fakes of the capture traits, for tests.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Mutex, MutexGuard};

use timereel_image::{Frame, RgbaImage};

use crate::{ActiveWindow, ActiveWindowSource, CaptureError, LockSource, ScreenSource};

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    // A fake is only used in tests; a poisoned lock still holds valid data.
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// A solid-colour frame at `(x, y)`, handy for building fake monitors.
pub fn solid_frame(x: i32, y: i32, width: u32, height: u32, rgba: [u8; 4]) -> Frame {
    let data = rgba.repeat((width * height) as usize);
    // The buffer has exactly width * height * 4 bytes, so this cannot fail
    // unless the size is zero, which is a bug in the test.
    let image = RgbaImage::new(width, height, data).expect("valid fake frame size");
    Frame::new(x, y, image)
}

/// A fake [`ScreenSource`]. It returns the queued results in order; the last
/// one is repeated once the queue has only one entry left.
pub struct FakeScreen {
    results: Mutex<VecDeque<Result<Vec<Frame>, CaptureError>>>,
    calls: AtomicUsize,
}

impl FakeScreen {
    /// A screen that always returns `frames`.
    pub fn with_frames(frames: Vec<Frame>) -> Self {
        Self::scripted(vec![Ok(frames)])
    }

    /// A screen with one 64x36 monitor at the origin.
    pub fn single_monitor() -> Self {
        Self::with_frames(vec![solid_frame(0, 0, 64, 36, [30, 90, 160, 255])])
    }

    /// A screen that always fails with `error`.
    pub fn failing(error: CaptureError) -> Self {
        Self::scripted(vec![Err(error)])
    }

    /// A screen that returns the given results in order (the last repeats).
    pub fn scripted(results: Vec<Result<Vec<Frame>, CaptureError>>) -> Self {
        Self {
            results: Mutex::new(results.into()),
            calls: AtomicUsize::new(0),
        }
    }

    /// How many times [`ScreenSource::capture`] was called.
    pub fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl ScreenSource for FakeScreen {
    fn capture(&self) -> Result<Vec<Frame>, CaptureError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let mut q = lock(&self.results);
        let next = if q.len() > 1 {
            q.pop_front()
        } else {
            q.front().cloned()
        };
        next.unwrap_or(Err(CaptureError::NoDisplay))
    }
}

/// A fake [`ActiveWindowSource`] that returns the queued answers in order;
/// the last one is repeated.
pub struct FakeWindow {
    answers: Mutex<VecDeque<ActiveWindow>>,
    calls: AtomicUsize,
}

impl FakeWindow {
    /// A source that always answers `window`.
    pub fn always(window: ActiveWindow) -> Self {
        Self::sequence(vec![window])
    }

    /// A source that answers [`ActiveWindow::Known`] with `app` and `title`.
    pub fn known(app: &str, title: &str) -> Self {
        Self::always(ActiveWindow::Known {
            app: app.into(),
            title: title.into(),
        })
    }

    /// A source that answers with the given windows in order (the last
    /// repeats). Use it to change the active app during a capture.
    pub fn sequence(windows: Vec<ActiveWindow>) -> Self {
        Self {
            answers: Mutex::new(windows.into()),
            calls: AtomicUsize::new(0),
        }
    }

    /// How many times [`ActiveWindowSource::active_window`] was called.
    pub fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl ActiveWindowSource for FakeWindow {
    fn active_window(&self) -> ActiveWindow {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let mut q = lock(&self.answers);
        let next = if q.len() > 1 {
            q.pop_front()
        } else {
            q.front().cloned()
        };
        next.unwrap_or(ActiveWindow::Unknown)
    }
}

/// A fake [`LockSource`] with a switch.
#[derive(Default)]
pub struct FakeLock {
    locked: AtomicBool,
}

impl FakeLock {
    /// A fake that reports `locked`.
    pub fn new(locked: bool) -> Self {
        Self {
            locked: AtomicBool::new(locked),
        }
    }

    /// Changes the answer.
    pub fn set_locked(&self, locked: bool) {
        self.locked.store(locked, Ordering::SeqCst);
    }
}

impl LockSource for FakeLock {
    fn is_locked(&self) -> bool {
        self.locked.load(Ordering::SeqCst)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_sequence_repeats_last() {
        let w = FakeWindow::sequence(vec![ActiveWindow::NoWindow, ActiveWindow::Unknown]);
        assert_eq!(w.active_window(), ActiveWindow::NoWindow);
        assert_eq!(w.active_window(), ActiveWindow::Unknown);
        assert_eq!(w.active_window(), ActiveWindow::Unknown);
        assert_eq!(w.calls(), 3);
    }

    #[test]
    fn screen_script_and_calls() {
        let s = FakeScreen::scripted(vec![Err(CaptureError::Portal), Ok(vec![])]);
        assert_eq!(s.capture().err(), Some(CaptureError::Portal));
        assert!(s.capture().is_ok());
        assert!(s.capture().is_ok());
        assert_eq!(s.calls(), 3);
    }

    #[test]
    fn lock_switch() {
        let l = FakeLock::new(false);
        assert!(!l.is_locked());
        l.set_locked(true);
        assert!(l.is_locked());
    }
}
