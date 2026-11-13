//! The `timereel` viewer (architecture section 2.4): the window shell, the
//! single-instance D-Bus service and the first-run set-up, the lock screen,
//! unlock, auto-lock and the Settings screen ([`settings`]).
//!
//! `timereel purge-data` lives in [`purge`].
//!
//! After set-up the viewer shows the lock screen ([`lock_ui`]); an unlock
//! creates a [`session::Session`] and the timeline ([`timeline_ui`]).

pub mod agent_client;
pub mod app;
pub mod autolock;
pub mod lock_ui;
pub mod lockwatch;
pub mod purge;
pub mod session;
pub mod settings;
pub mod setup;
pub mod setup_ui;
pub mod single;
pub mod sys;
pub mod timeline_ui;
