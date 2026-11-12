//! The quick-jump buttons.

use chrono::{Days, NaiveDate, TimeZone};
use timereel_store::CaptureTime;

use crate::index::{Index, Landing};
use crate::localday::{local_date, local_day_start};

const HOUR_MS: i64 = 3_600_000;

/// The kinds of quick jump.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuickJump {
    /// The start of today (the local day of `now`).
    Today,
    /// The start of the local day before today.
    Yesterday,
    /// The start of the local day before the current capture's day.
    PreviousDay,
    /// The start of the local day after the current capture's day.
    NextDay,
    /// One real hour earlier than the current capture.
    MinusHour,
    /// One real hour later than the current capture.
    PlusHour,
}

impl Index {
    /// Where a quick jump lands.
    ///
    /// * `current` is the capture on screen, `now` is the clock, and `tz` is
    ///   the local time zone.
    /// * The day jumps aim at local midnight of the target day and use
    ///   [`Index::at_or_after`], so on a day with no captures they land on
    ///   the next capture after it, or the last capture if none follows.
    ///   `exact` is `true` only if the capture is on the target day; the UI
    ///   can show 'showing nearest' otherwise.
    /// * The hour jumps move by 3,600,000 ms of real time, also across a DST
    ///   change. [`QuickJump::PlusHour`] lands on the first capture at or
    ///   after that moment and [`QuickJump::MinusHour`] on the last capture
    ///   at or before it; with none that way they fall back to the nearest
    ///   capture (`exact: false`).
    ///
    /// `None` only when the index is empty.
    pub fn quick_jump<Tz: TimeZone>(
        &self,
        kind: QuickJump,
        current: CaptureTime,
        now: CaptureTime,
        tz: &Tz,
    ) -> Option<Landing> {
        match kind {
            QuickJump::Today => self.jump_to_day(local_date(now, tz), tz),
            QuickJump::Yesterday => {
                self.jump_to_day(shift_days(local_date(now, tz), Direction::Back), tz)
            }
            QuickJump::PreviousDay => {
                self.jump_to_day(shift_days(local_date(current, tz), Direction::Back), tz)
            }
            QuickJump::NextDay => {
                self.jump_to_day(shift_days(local_date(current, tz), Direction::Forward), tz)
            }
            QuickJump::MinusHour => {
                self.at_or_before(CaptureTime(current.0.saturating_sub(HOUR_MS)))
            }
            QuickJump::PlusHour => self.at_or_after(CaptureTime(current.0.saturating_add(HOUR_MS))),
        }
    }

    fn jump_to_day<Tz: TimeZone>(&self, day: NaiveDate, tz: &Tz) -> Option<Landing> {
        let landing = self.at_or_after(local_day_start(day, tz))?;
        Some(Landing {
            exact: landing.exact && local_date(landing.time, tz) == day,
            time: landing.time,
        })
    }
}

enum Direction {
    Back,
    Forward,
}

fn shift_days(date: NaiveDate, dir: Direction) -> NaiveDate {
    match dir {
        Direction::Back => date.checked_sub_days(Days::new(1)),
        Direction::Forward => date.checked_add_days(Days::new(1)),
    }
    .unwrap_or(date)
}
