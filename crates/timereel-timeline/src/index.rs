//! The in-memory time index and its queries.

use std::time::Duration;

use chrono::{NaiveDate, TimeZone};
use timereel_store::{CaptureTime, Store, StoreError};

use crate::gaps::{find_gaps, Gap};
use crate::localday::local_day_bounds;

/// How often the viewer should call [`Index::refresh`] while it is unlocked.
pub const REFRESH_INTERVAL: Duration = Duration::from_secs(60);

/// Milliseconds in one UTC day (the unit of the day folders on disk).
const MS_PER_UTC_DAY: i64 = 86_400_000;

/// The answer of a query that may have to fall back to another capture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Landing {
    /// The capture found.
    pub time: CaptureTime,
    /// `true` if the capture was found as asked. `false` if the query had to
    /// fall back to the nearest capture, so the UI can say 'showing nearest'.
    pub exact: bool,
}

/// The sorted list of all capture times.
///
/// It is built from file names only, so it needs no key. Times are unique
/// and sorted oldest first.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Index {
    times: Vec<CaptureTime>,
}

impl Index {
    /// An empty index.
    pub fn new() -> Index {
        Index::default()
    }

    /// Builds an index from any list of times (sorted and de-duplicated).
    pub fn from_times(mut times: Vec<CaptureTime>) -> Index {
        times.sort_unstable();
        times.dedup();
        Index { times }
    }

    /// Lists every capture in `store` and builds the index (a full load).
    pub fn load(store: &Store) -> Result<Index, StoreError> {
        Ok(Index::from_times(store.list(..)?))
    }

    /// Picks up new captures without listing the whole store.
    ///
    /// Only the day folder of the newest known capture and the folders after
    /// it are listed (normally just today's folder). Captures from earlier
    /// days are kept as they are, so a capture deleted by retention stays in
    /// the index until the next [`Index::load`]. Captures that vanished from
    /// the re-listed folders are dropped. On an empty index everything is
    /// listed. On an error the index is not changed.
    ///
    /// Returns how many captures are new.
    pub fn refresh(&mut self, store: &Store) -> Result<usize, StoreError> {
        let Some(last) = self.times.last() else {
            let all = store.list(..)?;
            let added = all.len();
            self.times = all;
            return Ok(added);
        };
        let cut = CaptureTime(last.day() * MS_PER_UTC_DAY);
        let keep = self.times.partition_point(|t| *t < cut);
        let listed = store.list(cut..)?;
        let old_tail = &self.times[keep..];
        let added = listed
            .iter()
            .filter(|t| old_tail.binary_search(t).is_err())
            .count();
        self.times.truncate(keep);
        self.times.extend(listed);
        Ok(added)
    }

    /// All capture times, oldest first.
    pub fn times(&self) -> &[CaptureTime] {
        &self.times
    }

    /// How many captures there are.
    pub fn len(&self) -> usize {
        self.times.len()
    }

    /// Whether there are no captures.
    pub fn is_empty(&self) -> bool {
        self.times.is_empty()
    }

    /// The oldest capture.
    pub fn first(&self) -> Option<CaptureTime> {
        self.times.first().copied()
    }

    /// The newest capture.
    pub fn last(&self) -> Option<CaptureTime> {
        self.times.last().copied()
    }

    /// The first capture strictly after `t`. `t` need not be in the index.
    pub fn next(&self, t: CaptureTime) -> Option<CaptureTime> {
        let i = self.times.partition_point(|x| *x <= t);
        self.times.get(i).copied()
    }

    /// The last capture strictly before `t`. `t` need not be in the index.
    pub fn prev(&self, t: CaptureTime) -> Option<CaptureTime> {
        let i = self.times.partition_point(|x| *x < t);
        i.checked_sub(1).and_then(|i| self.times.get(i)).copied()
    }

    /// The captures in `[from, to)`, oldest first.
    pub fn between(&self, from: CaptureTime, to: CaptureTime) -> &[CaptureTime] {
        let start = self.times.partition_point(|x| *x < from);
        let end = self.times.partition_point(|x| *x < to).max(start);
        &self.times[start..end]
    }

    /// The captures of the local day `day` in `tz`, oldest first.
    ///
    /// The day is cut at local midnight, so it can reach into two UTC day
    /// folders, and it is 23 or 25 hours long on a DST change.
    pub fn range<Tz: TimeZone>(&self, day: NaiveDate, tz: &Tz) -> &[CaptureTime] {
        let (from, to) = local_day_bounds(day, tz);
        self.between(from, to)
    }

    /// The first capture at or after `t`. If none follows, falls back to the
    /// nearest capture (the last one) and reports `exact: false`. `None`
    /// only when the index is empty.
    pub fn at_or_after(&self, t: CaptureTime) -> Option<Landing> {
        let i = self.times.partition_point(|x| *x < t);
        match self.times.get(i) {
            Some(&time) => Some(Landing { time, exact: true }),
            None => self.last().map(|time| Landing { time, exact: false }),
        }
    }

    /// The last capture at or before `t`. If none is that early, falls back
    /// to the nearest capture (the first one) and reports `exact: false`.
    pub(crate) fn at_or_before(&self, t: CaptureTime) -> Option<Landing> {
        let i = self.times.partition_point(|x| *x <= t);
        match i.checked_sub(1).and_then(|i| self.times.get(i)) {
            Some(&time) => Some(Landing { time, exact: true }),
            None => self.first().map(|time| Landing { time, exact: false }),
        }
    }

    /// The capture closest in time to `t`. On a tie the earlier one wins.
    pub fn nearest(&self, t: CaptureTime) -> Option<CaptureTime> {
        let i = self.times.partition_point(|x| *x < t);
        let after = self.times.get(i).copied();
        let before = i.checked_sub(1).and_then(|i| self.times.get(i)).copied();
        match (before, after) {
            (Some(b), Some(a)) => Some(if t.0 - b.0 <= a.0 - t.0 { b } else { a }),
            (b, a) => b.or(a),
        }
    }

    /// The breaks between neighbouring captures that are longer than
    /// 1.5 x `interval`. For one day, use [`find_gaps`] on [`Index::range`].
    pub fn gaps(&self, interval: Duration) -> Vec<Gap> {
        find_gaps(&self.times, interval)
    }
}
