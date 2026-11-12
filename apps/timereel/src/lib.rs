//! The `timereel` viewer (architecture section 2.4): the window shell, the
//! single-instance D-Bus service and the first-run set-up.
//!
//! The lock screen and everything after it come with later tickets; until then
//! an already set-up installation sees a placeholder.

pub mod app;
pub mod setup;
pub mod setup_ui;
pub mod single;
pub mod sys;
