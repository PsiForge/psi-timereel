//! The `timereel` viewer (architecture section 2.4): the window shell, the
//! single-instance D-Bus service and the first-run set-up, the lock screen,
//! unlock and auto-lock.
//!
//! `timereel purge-data` lives in [`purge`].
//!
//! After set-up the viewer shows the lock screen ([`lock_ui`]); an unlock
//! creates a [`session::Session`]. The timeline is still a placeholder.

pub mod app;
pub mod autolock;
pub mod lock_ui;
pub mod lockwatch;
pub mod purge;
pub mod session;
pub mod setup;
pub mod setup_ui;
pub mod single;
pub mod sys;
