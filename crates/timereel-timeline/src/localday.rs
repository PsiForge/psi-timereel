//! Local day boundaries with chrono.

use chrono::{DateTime, Days, NaiveDate, NaiveDateTime, NaiveTime, TimeZone};
use timereel_store::CaptureTime;

/// The local calendar date of `time` in `tz`.
///
/// A time outside chrono's range (not possible for a capture file name) is
/// treated as 1970-01-01.
pub fn local_date<Tz: TimeZone>(time: CaptureTime, tz: &Tz) -> NaiveDate {
    DateTime::from_timestamp_millis(time.0)
        .map(|utc| utc.with_timezone(tz).date_naive())
        .unwrap_or_default()
}

/// The first instant of the local day `date` in `tz`.
///
/// Normally this is local midnight. If midnight does not exist (a DST change
/// at midnight), it is the first moment that does exist on that date. If it
/// happens twice, the earlier one is used.
pub fn local_day_start<Tz: TimeZone>(date: NaiveDate, tz: &Tz) -> CaptureTime {
    let midnight = date.and_time(NaiveTime::MIN);
    if let Some(t) = resolve(midnight, tz) {
        return t;
    }
    // Midnight falls into a DST gap: walk forward to the end of the gap.
    (1..=24 * 60)
        .find_map(|minutes| resolve(midnight + chrono::Duration::minutes(minutes), tz))
        .unwrap_or_else(|| CaptureTime(midnight.and_utc().timestamp_millis()))
}

fn resolve<Tz: TimeZone>(local: NaiveDateTime, tz: &Tz) -> Option<CaptureTime> {
    tz.from_local_datetime(&local)
        .earliest()
        .map(|t| CaptureTime(t.timestamp_millis()))
}

/// The local day `date` as a half-open range `[start, end)`. The end is the
/// start of the next local day, so the range is 23 or 25 hours long on a DST
/// change day.
pub fn local_day_bounds<Tz: TimeZone>(date: NaiveDate, tz: &Tz) -> (CaptureTime, CaptureTime) {
    let next = date.checked_add_days(Days::new(1)).unwrap_or(date);
    (local_day_start(date, tz), local_day_start(next, tz))
}
