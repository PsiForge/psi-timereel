//! The clock the agent schedules with.
//!
//! Scheduling is by wall-clock time (architecture section 2.2): the agent
//! sleeps until a Unix time, and checks the wall clock again when it wakes.
//! That keeps the schedule right after a suspend, where the monotonic clock
//! that `tokio` uses stands still. [`FakeClock`] lets tests move time by hand.

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use tokio::sync::watch;

use crate::cycle::Now;

/// A boxed future that is ready when a deadline has passed.
pub type Sleep<'a> = Pin<Box<dyn Future<Output = ()> + Send + 'a>>;

/// A source of wall-clock time and of sleeping until a wall-clock time.
pub trait Clock: Send + Sync + 'static {
    /// The current time.
    fn now(&self) -> Now;

    /// Completes when the wall clock has reached `deadline_ms` (Unix ms).
    fn sleep_until(&self, deadline_ms: i64) -> Sleep<'_>;
}

/// The real clock: `SystemTime` and `tokio::time`.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemClock;

fn system_unix_ms() -> i64 {
    match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        Ok(d) => i64::try_from(d.as_millis()).unwrap_or(i64::MAX),
        Err(e) => -i64::try_from(e.duration().as_millis()).unwrap_or(i64::MAX),
    }
}

impl Clock for SystemClock {
    fn now(&self) -> Now {
        let unix_ms = system_unix_ms();
        Now {
            unix_ms,
            tz_offset_min: crate::sys::local_offset_min(unix_ms),
        }
    }

    fn sleep_until(&self, deadline_ms: i64) -> Sleep<'_> {
        Box::pin(async move {
            // The monotonic timer may be early or late (suspend, clock
            // changes), so check the wall clock again after every sleep.
            loop {
                let remaining = deadline_ms.saturating_sub(system_unix_ms());
                if remaining <= 0 {
                    return;
                }
                let ms = u64::try_from(remaining).unwrap_or(0);
                tokio::time::sleep(Duration::from_millis(ms)).await;
            }
        })
    }
}

/// A clock that only moves when [`FakeClock::advance`] or
/// [`FakeClock::set`] is called. It counts how often a sleep ended, so tests
/// can show that the agent does not wake between scheduled times.
#[derive(Debug)]
pub struct FakeClock {
    time: watch::Sender<i64>,
    tz_offset_min: i32,
    wakeups: AtomicUsize,
}

impl FakeClock {
    /// A fake clock that starts at `unix_ms` with a UTC local time.
    pub fn new(unix_ms: i64) -> Self {
        Self {
            time: watch::channel(unix_ms).0,
            tz_offset_min: 0,
            wakeups: AtomicUsize::new(0),
        }
    }

    /// Sets the time.
    pub fn set(&self, unix_ms: i64) {
        self.time.send_replace(unix_ms);
    }

    /// Moves the time forward by `ms`.
    pub fn advance(&self, ms: i64) {
        self.time.send_modify(|t| *t += ms);
    }

    /// The current fake time in Unix ms.
    pub fn unix_ms(&self) -> i64 {
        *self.time.borrow()
    }

    /// How many sleeps have ended because their deadline was reached.
    pub fn wakeups(&self) -> usize {
        self.wakeups.load(Ordering::SeqCst)
    }
}

impl Clock for FakeClock {
    fn now(&self) -> Now {
        Now {
            unix_ms: self.unix_ms(),
            tz_offset_min: self.tz_offset_min,
        }
    }

    fn sleep_until(&self, deadline_ms: i64) -> Sleep<'_> {
        let mut rx = self.time.subscribe();
        Box::pin(async move {
            loop {
                if *rx.borrow_and_update() >= deadline_ms {
                    self.wakeups.fetch_add(1, Ordering::SeqCst);
                    return;
                }
                if rx.changed().await.is_err() {
                    // The clock is gone: never wake.
                    std::future::pending::<()>().await;
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(flavor = "current_thread")]
    async fn fake_sleep_ends_only_when_the_deadline_is_reached() {
        let clock = FakeClock::new(1_000);
        let sleep = clock.sleep_until(5_000);
        tokio::pin!(sleep);
        clock.advance(1_000);
        let early = tokio::time::timeout(Duration::from_millis(20), &mut sleep).await;
        assert!(early.is_err(), "woke before the deadline");
        assert_eq!(clock.wakeups(), 0);
        clock.advance(3_000);
        sleep.await;
        assert_eq!(clock.wakeups(), 1);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn system_sleep_returns_at_once_for_a_past_deadline() {
        let clock = SystemClock;
        clock.sleep_until(0).await;
        let now = clock.now();
        assert!(now.unix_ms > 1_700_000_000_000);
    }
}
