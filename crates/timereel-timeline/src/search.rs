//! In-memory search over the decrypted metadata.
//!
//! A [`SearchFilter`] is a case-insensitive substring test on the app name
//! and the window title. It works on the metadata that the [`MetaStore`] has
//! loaded so far: a capture whose details are not loaded yet (or could not be
//! read) does not match a non-empty query, and shows up when the loading
//! reaches it. An empty query matches every capture.
//!
//! Nothing here touches the disk, and the query text is never logged (the
//! `Debug` output of a filter hides it).

use std::fmt;

use timereel_store::{CaptureMeta, CaptureTime};

use crate::index::Index;
use crate::meta::{MetaEntry, MetaStore};

/// A search query, ready to test metadata against.
#[derive(Clone, PartialEq, Eq)]
pub struct SearchFilter {
    /// The query in Unicode lowercase.
    needle: String,
}

impl fmt::Debug for SearchFilter {
    /// The query may be private, so it is not printed.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SearchFilter")
            .field("empty", &self.needle.is_empty())
            .finish()
    }
}

impl SearchFilter {
    /// A filter for `query`. Case is ignored using Unicode lowercase, so
    /// non-ASCII letters such as `Ä` and `ä` are equal.
    pub fn new(query: &str) -> Self {
        Self {
            needle: query.to_lowercase(),
        }
    }

    /// Whether the query is empty (it then matches everything).
    pub fn is_empty(&self) -> bool {
        self.needle.is_empty()
    }

    /// Whether `meta` matches: the query is a substring of the app name or
    /// of the window title, ignoring case.
    pub fn matches(&self, meta: &CaptureMeta) -> bool {
        self.is_empty()
            || meta.app_name.to_lowercase().contains(&self.needle)
            || meta.window_title.to_lowercase().contains(&self.needle)
    }
}

impl MetaStore {
    /// Whether the capture at `time` matches `filter`. With an empty query
    /// every capture matches. Otherwise only a capture whose metadata is
    /// loaded (and readable) can match.
    pub fn matches(&self, time: CaptureTime, filter: &SearchFilter) -> bool {
        if filter.is_empty() {
            return true;
        }
        match self.get(time) {
            Some(MetaEntry::Ready(meta)) => filter.matches(&meta),
            Some(MetaEntry::Failed(_)) | None => false,
        }
    }

    /// The times in `times` (kept in their order) that match `filter`.
    pub fn matching(&self, times: &[CaptureTime], filter: &SearchFilter) -> Vec<CaptureTime> {
        times
            .iter()
            .copied()
            .filter(|t| self.matches(*t, filter))
            .collect()
    }
}

impl Index {
    /// The first capture strictly after `t` for which `keep` is true, so
    /// that navigation skips captures that do not match a search. `t` need
    /// not be in the index or match itself.
    pub fn next_matching(
        &self,
        t: CaptureTime,
        keep: impl Fn(CaptureTime) -> bool,
    ) -> Option<CaptureTime> {
        let times = self.times();
        let i = times.partition_point(|x| *x <= t);
        times[i..].iter().copied().find(|x| keep(*x))
    }

    /// The last capture strictly before `t` for which `keep` is true.
    pub fn prev_matching(
        &self,
        t: CaptureTime,
        keep: impl Fn(CaptureTime) -> bool,
    ) -> Option<CaptureTime> {
        let times = self.times();
        let i = times.partition_point(|x| *x < t);
        times[..i].iter().rev().copied().find(|x| keep(*x))
    }
}
