//! Breaks in the recording.

use std::time::Duration;

use timereel_store::CaptureTime;

/// A break is longer than this many percent of the capture interval (150 = 1.5 x).
pub const GAP_FACTOR_PERCENT: i128 = 150;

/// A break between two neighbouring captures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Gap {
    /// The last capture before the break.
    pub before: CaptureTime,
    /// The first capture after the break.
    pub after: CaptureTime,
}

impl Gap {
    /// How long the break is, in milliseconds.
    pub fn length_ms(&self) -> i64 {
        self.after.0.saturating_sub(self.before.0)
    }
}

/// Finds the breaks in `times` (sorted, oldest first): two neighbours that
/// are more than 1.5 x `interval` apart. Exactly 1.5 x is not a break.
pub fn find_gaps(times: &[CaptureTime], interval: Duration) -> Vec<Gap> {
    let interval_ms = i128::try_from(interval.as_millis()).unwrap_or(i128::MAX / 1000);
    times
        .windows(2)
        .filter(|w| {
            let delta = i128::from(w[1].0) - i128::from(w[0].0);
            delta * 100 > interval_ms * GAP_FACTOR_PERCENT
        })
        .map(|w| Gap {
            before: w[0],
            after: w[1],
        })
        .collect()
}
