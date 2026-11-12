//! UTC capture times and their file and folder names.
//!
//! A capture time is a count of milliseconds since the Unix epoch (UTC). The
//! file name is `YYYYMMDDTHHMMSSmmmZ` and the day folder is `YYYY-MM-DD`.
//! The calendar maths is done here (days <-> civil date) so no date crate is
//! needed.

use std::fmt;

/// Milliseconds in one day.
pub const MS_PER_DAY: i64 = 86_400_000;

/// A capture time: milliseconds since the Unix epoch, UTC.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CaptureTime(pub i64);

/// Days since 1970-01-01 for a civil date (proleptic Gregorian calendar).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Civil date `(year, month, day)` for days since 1970-01-01.
fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

impl CaptureTime {
    /// The UTC day number (days since 1970-01-01) of this time.
    pub fn day(self) -> i64 {
        self.0.div_euclid(MS_PER_DAY)
    }

    /// Whether the time can be written as a four-digit-year file name.
    pub fn is_representable(self) -> bool {
        let (y, _, _) = civil_from_days(self.day());
        (0..=9999).contains(&y)
    }

    /// The day folder name, `YYYY-MM-DD` (UTC).
    pub fn day_name(self) -> String {
        day_name(self.day())
    }

    /// The file stem, `YYYYMMDDTHHMMSSmmmZ` (UTC).
    pub fn file_stem(self) -> String {
        let (y, mo, d) = civil_from_days(self.day());
        let ms_of_day = self.0.rem_euclid(MS_PER_DAY);
        let (h, mi, s, ms) = (
            ms_of_day / 3_600_000,
            ms_of_day / 60_000 % 60,
            ms_of_day / 1_000 % 60,
            ms_of_day % 1_000,
        );
        format!("{y:04}{mo:02}{d:02}T{h:02}{mi:02}{s:02}{ms:03}Z")
    }

    /// Parses a file stem. Only the exact canonical form is accepted.
    pub fn parse_stem(stem: &str) -> Option<CaptureTime> {
        let b = stem.as_bytes();
        if b.len() != 19 || b[8] != b'T' || b[18] != b'Z' {
            return None;
        }
        let digits = |range: std::ops::Range<usize>| -> Option<i64> {
            let part = &b[range];
            if part.iter().all(u8::is_ascii_digit) {
                part.iter()
                    .try_fold(0i64, |acc, d| Some(acc * 10 + i64::from(d - b'0')))
            } else {
                None
            }
        };
        let (y, mo, d) = (digits(0..4)?, digits(4..6)?, digits(6..8)?);
        let (h, mi, s, ms) = (
            digits(9..11)?,
            digits(11..13)?,
            digits(13..15)?,
            digits(15..18)?,
        );
        if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || s > 59 {
            return None;
        }
        let t = CaptureTime(
            days_from_civil(y, mo, d) * MS_PER_DAY + h * 3_600_000 + mi * 60_000 + s * 1_000 + ms,
        );
        // Rejects dates that do not exist, such as February 30th.
        (t.file_stem() == stem).then_some(t)
    }
}

/// The folder name `YYYY-MM-DD` for a UTC day number.
pub fn day_name(day: i64) -> String {
    let (y, m, d) = civil_from_days(day);
    format!("{y:04}-{m:02}-{d:02}")
}

/// Parses a folder name `YYYY-MM-DD` into a UTC day number.
pub fn parse_day_name(name: &str) -> Option<i64> {
    let b = name.as_bytes();
    if b.len() != 10 || b[4] != b'-' || b[7] != b'-' {
        return None;
    }
    let num = |r: std::ops::Range<usize>| -> Option<i64> {
        name.get(r)
            .filter(|s| s.bytes().all(|c| c.is_ascii_digit()))?
            .parse()
            .ok()
    };
    let (y, m, d) = (num(0..4)?, num(5..7)?, num(8..10)?);
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    let day = days_from_civil(y, m, d);
    (day_name(day) == name).then_some(day)
}

impl fmt::Display for CaptureTime {
    /// Formats as `2025-05-14 09:30:00.123 UTC`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let stem = self.file_stem();
        write!(
            f,
            "{}-{}-{} {}:{}:{}.{} UTC",
            &stem[0..4],
            &stem[4..6],
            &stem[6..8],
            &stem[9..11],
            &stem[11..13],
            &stem[13..15],
            &stem[15..18]
        )
    }
}
