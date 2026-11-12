//! `timereel-agent` library: the agent without its `main`, so that it can be
//! tested with fake sources and a fake clock.
//!
//! * [`cycle`]: one capture cycle and the state machine;
//! * [`runtime`]: the main loop (schedule, live config, pause, retention);
//! * [`service`]: the `org.timereel.Agent1` D-Bus interface;
//! * [`logind`]: reschedule after suspend;
//! * [`clock`], [`sources`], [`logging`], [`sys`]: the clock, the real
//!   (for now unsupported) sources, journal logging and system calls.

pub mod clock;
pub mod cycle;
pub mod logging;
pub mod logind;
pub mod runtime;
pub mod service;
pub mod sources;
pub mod sys;
