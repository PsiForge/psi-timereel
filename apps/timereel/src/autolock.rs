//! Auto-lock after a time without input (`ui_autolock_minutes`).
//!
//! The time comes from a [`Clock`], so tests can use a [`FakeClock`].

use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

/// A source of the current time.
pub trait Clock: Send {
    /// The current monotonic time.
    fn now(&self) -> Instant;
}

/// The real monotonic clock.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Instant {
        Instant::now()
    }
}

/// A clock that only moves when told to. Clones share the same time.
#[derive(Debug, Clone)]
pub struct FakeClock(Arc<Mutex<Instant>>);

impl Default for FakeClock {
    fn default() -> Self {
        Self(Arc::new(Mutex::new(Instant::now())))
    }
}

impl FakeClock {
    /// Moves the clock forward.
    pub fn advance(&self, by: Duration) {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner) += by;
    }
}

impl Clock for FakeClock {
    fn now(&self) -> Instant {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The idle time after which the viewer locks, from `ui_autolock_minutes`.
pub fn autolock_after(minutes: u32) -> Duration {
    Duration::from_secs(u64::from(minutes) * 60)
}

/// Tracks the time since the last input.
#[derive(Debug, Clone)]
pub struct IdleTimer {
    timeout: Duration,
    last_input: Instant,
}

impl IdleTimer {
    /// A timer that starts counting at `now`.
    pub fn new(timeout: Duration, now: Instant) -> Self {
        Self {
            timeout,
            last_input: now,
        }
    }

    /// Records input at `now`; the countdown starts again.
    pub fn input(&mut self, now: Instant) {
        self.last_input = now;
    }

    /// True once there has been no input for the whole timeout.
    pub fn expired(&self, now: Instant) -> bool {
        self.remaining(now).is_zero()
    }

    /// The time left until [`IdleTimer::expired`] becomes true.
    pub fn remaining(&self, now: Instant) -> Duration {
        self.timeout
            .saturating_sub(now.saturating_duration_since(self.last_input))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_input_for_the_configured_time_expires() {
        let clock = FakeClock::default();
        let timer = IdleTimer::new(autolock_after(10), clock.now());
        clock.advance(Duration::from_secs(9 * 60 + 59));
        assert!(!timer.expired(clock.now()));
        assert_eq!(timer.remaining(clock.now()), Duration::from_secs(1));
        clock.advance(Duration::from_secs(1));
        assert!(timer.expired(clock.now()));
    }

    #[test]
    fn input_resets_the_timer() {
        let clock = FakeClock::default();
        let mut timer = IdleTimer::new(autolock_after(1), clock.now());
        clock.advance(Duration::from_secs(50));
        timer.input(clock.now());
        clock.advance(Duration::from_secs(50));
        assert!(!timer.expired(clock.now()));
        clock.advance(Duration::from_secs(10));
        assert!(timer.expired(clock.now()));
    }
}
