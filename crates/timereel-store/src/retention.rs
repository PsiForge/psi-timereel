//! Retention: deletes captures older than the configured number of days.
//!
//! Retention works from file and folder names only. It never opens a file and
//! needs no key (ADR 0006).
//!
//! # Rules
//!
//! * A capture is deleted when its time is **earlier than** `now - days x
//!   24 h`. A capture exactly at that boundary is kept.
//! * Only regular files named like a capture (`<stem>.trc`) or an interrupted
//!   write (`.<stem>.partial`) inside a UTC day folder of the same day are
//!   deleted. The time of a `.partial` file is the time in its name.
//! * Symlinks are never followed and never deleted: every entry is checked
//!   with `symlink_metadata`, and each path is checked to lie inside the
//!   canonical captures root before it is touched.
//! * Day folders that are empty afterwards are removed.
//! * Entries with unknown names are left alone and logged (the log has the
//!   entry kind, never the name or any content).
//! * A delete that fails is logged with the kind of the path and the system
//!   error kind, counted in [`Report::failed`], and tried again at the next
//!   run, because the file is still there.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use tracing::warn;

use crate::store::{Store, CAPTURE_EXT, PARTIAL_EXT};
use crate::time::{parse_day_name, CaptureTime, MS_PER_DAY};

/// The outcome of one retention run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Report {
    /// Capture and `.partial` files that were deleted.
    pub deleted: usize,
    /// Deletes (files or empty folders) or folder reads that failed. They
    /// are retried at the next run.
    pub failed: usize,
}

/// What kind of file a name denotes.
#[derive(Clone, Copy)]
enum Kind {
    Capture,
    Partial,
}

impl Kind {
    fn label(self) -> &'static str {
        match self {
            Kind::Capture => "capture",
            Kind::Partial => "partial",
        }
    }
}

/// Deletes every capture older than `days` days before `now`.
///
/// `now` is passed in so tests can use a fixed clock. A missing captures
/// folder is an empty store. Problems are logged and counted in the report;
/// this function does not fail and does not panic.
pub fn run(store: &Store, now: CaptureTime, days: u32) -> Report {
    walk(store, cutoff(now, days), true, &mut |path| {
        fs::remove_file(path)
    })
}

/// How many files [`run`] would delete with the same arguments, without
/// deleting anything. It counts `.trc` and `.partial` files, like `run`.
pub fn count_older_than(store: &Store, now: CaptureTime, days: u32) -> usize {
    walk(store, cutoff(now, days), false, &mut |_| Ok(())).deleted
}

/// Times strictly before the cutoff are deleted.
fn cutoff(now: CaptureTime, days: u32) -> i64 {
    now.0.saturating_sub(i64::from(days) * MS_PER_DAY)
}

/// Parses `<stem>.trc` or `.<stem>.partial`.
fn parse_name(name: &str) -> Option<(Kind, CaptureTime)> {
    if let Some(stem) = name.strip_suffix(&format!(".{CAPTURE_EXT}")) {
        return CaptureTime::parse_stem(stem).map(|t| (Kind::Capture, t));
    }
    name.strip_prefix('.')
        .and_then(|n| n.strip_suffix(&format!(".{PARTIAL_EXT}")))
        .and_then(CaptureTime::parse_stem)
        .map(|t| (Kind::Partial, t))
}

/// `path` is strictly inside `root` (both canonical or built from canonical
/// parts, so a plain prefix check is enough).
fn is_inside(root: &Path, path: &Path) -> bool {
    path != root && path.starts_with(root)
}

fn log_failed(what: &'static str, kind: &str, err: &io::Error) {
    warn!(
        kind,
        error_kind = ?err.kind(),
        "retention: could not {what}; it is retried at the next run"
    );
}

/// Walks the captures folder and calls `delete` for each file that is old
/// enough. With `remove_empty_dirs` it also removes empty day folders.
fn walk(
    store: &Store,
    cutoff: i64,
    remove_empty_dirs: bool,
    delete: &mut dyn FnMut(&Path) -> io::Result<()>,
) -> Report {
    let mut report = Report::default();
    let root = match fs::canonicalize(store.captures_dir()) {
        Ok(root) => root,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return report,
        Err(e) => {
            log_failed("open the captures folder", "captures-root", &e);
            report.failed += 1;
            return report;
        }
    };
    let entries = match fs::read_dir(&root) {
        Ok(rd) => rd,
        Err(e) => {
            log_failed("list the captures folder", "captures-root", &e);
            report.failed += 1;
            return report;
        }
    };
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(e) => {
                log_failed("list the captures folder", "captures-root", &e);
                report.failed += 1;
                continue;
            }
        };
        let day = entry.file_name().to_str().and_then(parse_day_name);
        // `symlink_metadata` does not follow links: a link is never a folder.
        let is_dir = fs::symlink_metadata(entry.path()).is_ok_and(|m| m.file_type().is_dir());
        let (Some(day), true) = (day, is_dir) else {
            warn!("retention: ignoring an unknown entry in the captures folder");
            continue;
        };
        walk_day(
            &root,
            &entry.path(),
            day,
            cutoff,
            remove_empty_dirs,
            delete,
            &mut report,
        );
    }
    report
}

fn walk_day(
    root: &Path,
    day_path: &Path,
    day: i64,
    cutoff: i64,
    remove_empty_dirs: bool,
    delete: &mut dyn FnMut(&Path) -> io::Result<()>,
    report: &mut Report,
) {
    let dir: PathBuf = match fs::canonicalize(day_path) {
        Ok(dir) if is_inside(root, &dir) && dir.parent() == Some(root) => dir,
        Ok(_) => {
            warn!("retention: a day folder resolves outside the captures folder; ignored");
            return;
        }
        Err(e) => {
            log_failed("open a day folder", "day-folder", &e);
            report.failed += 1;
            return;
        }
    };
    let entries = match fs::read_dir(&dir) {
        Ok(rd) => rd,
        Err(e) => {
            log_failed("list a day folder", "day-folder", &e);
            report.failed += 1;
            return;
        }
    };
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(e) => {
                log_failed("list a day folder", "day-folder", &e);
                report.failed += 1;
                continue;
            }
        };
        let path = dir.join(entry.file_name());
        let regular = fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_file());
        let parsed = entry
            .file_name()
            .to_str()
            .and_then(parse_name)
            .filter(|(_, t)| t.day() == day);
        let (true, Some((kind, time))) = (regular, parsed) else {
            warn!("retention: ignoring an unknown entry in a day folder");
            continue;
        };
        if time.0 >= cutoff || !is_inside(root, &path) {
            continue;
        }
        match delete(&path) {
            Ok(()) => report.deleted += 1,
            // Gone already (for example removed by someone else): fine.
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => {
                log_failed("delete a file", kind.label(), &e);
                report.failed += 1;
            }
        }
    }
    if remove_empty_dirs {
        let empty = fs::read_dir(&dir).is_ok_and(|mut rd| rd.next().is_none());
        // `remove_dir` refuses a folder that is not empty, so a file that
        // appeared in the meantime is safe.
        if let (true, Err(e)) = (empty, fs::remove_dir(&dir)) {
            if e.kind() != io::ErrorKind::NotFound {
                log_failed("remove an empty day folder", "day-folder", &e);
                report.failed += 1;
            }
        }
    }
}
