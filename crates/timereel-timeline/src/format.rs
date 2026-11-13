//! Text for the timeline labels.

use chrono::{DateTime, FixedOffset};
use timereel_store::CaptureTime;

fn local(time: CaptureTime, tz_offset_min: i32) -> Option<DateTime<FixedOffset>> {
    let offset = FixedOffset::east_opt(tz_offset_min.checked_mul(60)?)?;
    Some(DateTime::from_timestamp_millis(time.0)?.with_timezone(&offset))
}

/// The local time of day, `HH:MM:SS`, at the UTC offset the capture recorded.
pub fn format_clock(time: CaptureTime, tz_offset_min: i32) -> String {
    local(time, tz_offset_min).map_or_else(
        || "--:--:--".to_string(),
        |t| t.format("%H:%M:%S").to_string(),
    )
}

/// The local date and time, `YYYY-MM-DD HH:MM:SS`.
pub fn format_date_time(time: CaptureTime, tz_offset_min: i32) -> String {
    local(time, tz_offset_min).map_or_else(
        || "unknown time".to_string(),
        |t| t.format("%Y-%m-%d %H:%M:%S").to_string(),
    )
}

/// The length of a break, for example `25 min`, `2 h 5 min` or `1 d 3 h`.
/// Rounded to the nearest minute.
pub fn format_gap(length_ms: i64) -> String {
    let minutes = (length_ms.max(0) + 30_000) / 60_000;
    let (days, hours, mins) = (minutes / 1440, minutes % 1440 / 60, minutes % 60);
    let part = |n: i64, unit: &str| (n > 0).then(|| format!("{n} {unit}"));
    let parts: Vec<String> = if days > 0 {
        [part(days, "d"), part(hours, "h")]
            .into_iter()
            .flatten()
            .collect()
    } else if hours > 0 {
        [part(hours, "h"), part(mins, "min")]
            .into_iter()
            .flatten()
            .collect()
    } else {
        vec![format!("{mins} min")]
    };
    parts.join(" ")
}
