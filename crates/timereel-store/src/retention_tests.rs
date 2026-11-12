#![allow(clippy::unwrap_used)]

use std::fs;
use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::{Path, PathBuf};

use crate::retention::{self, Report};
use crate::time::{CaptureTime, MS_PER_DAY};
use crate::Store;

// 2025-05-14T09:30:00.123Z
const NOW: i64 = 1_747_215_000_123;
const DAYS: u32 = 30;
const BOUNDARY: i64 = NOW - 30 * MS_PER_DAY;

struct Fx {
    dir: tempfile::TempDir,
    store: Store,
}

fn fx() -> Fx {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir(dir.path().join("data")).unwrap();
    let store = Store::new(dir.path().join("data"));
    fs::create_dir(store.captures_dir()).unwrap();
    Fx { dir, store }
}

impl Fx {
    /// Creates a capture file for `ms` with some content.
    fn capture(&self, ms: i64) -> PathBuf {
        self.file_in_day(ms, &format!("{}.trc", CaptureTime(ms).file_stem()))
    }

    fn partial(&self, ms: i64) -> PathBuf {
        self.file_in_day(ms, &format!(".{}.partial", CaptureTime(ms).file_stem()))
    }

    fn file_in_day(&self, ms: i64, name: &str) -> PathBuf {
        let dir = self.store.captures_dir().join(CaptureTime(ms).day_name());
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        fs::write(&path, b"x").unwrap();
        path
    }

    fn run(&self) -> Report {
        retention::run(&self.store, CaptureTime(NOW), DAYS)
    }

    fn count(&self) -> usize {
        retention::count_older_than(&self.store, CaptureTime(NOW), DAYS)
    }
}

#[test]
fn boundary_just_older_exact_and_newer() {
    let f = fx();
    let older = f.capture(BOUNDARY - 1);
    let exact = f.capture(BOUNDARY);
    let newer = f.capture(BOUNDARY + 1);
    let recent = f.capture(NOW);
    assert_eq!(
        f.run(),
        Report {
            deleted: 1,
            failed: 0
        }
    );
    assert!(!older.exists());
    assert!(exact.exists());
    assert!(newer.exists());
    assert!(recent.exists());
}

#[test]
fn boundary_works_across_day_folders() {
    // The boundary is mid-day: captures on the boundary day are decided by
    // their time, not by the folder.
    let f = fx();
    let before = f.capture(BOUNDARY - 60_000);
    let after = f.capture(BOUNDARY + 60_000);
    assert_eq!(f.run().deleted, 1);
    assert!(!before.exists());
    assert!(after.exists());
}

#[test]
fn zero_days_keeps_nothing_in_the_past_but_keeps_now() {
    let f = fx();
    let past = f.capture(NOW - 1);
    let now = f.capture(NOW);
    let r = retention::run(&f.store, CaptureTime(NOW), 0);
    assert_eq!(
        r,
        Report {
            deleted: 1,
            failed: 0
        }
    );
    assert!(!past.exists());
    assert!(now.exists());
}

#[test]
fn old_partial_files_are_deleted_new_ones_kept() {
    let f = fx();
    let old = f.partial(BOUNDARY - 1);
    let new = f.partial(NOW);
    assert_eq!(f.run().deleted, 1);
    assert!(!old.exists());
    assert!(new.exists());
}

#[test]
fn missing_captures_folder_is_empty() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path());
    assert_eq!(
        retention::run(&store, CaptureTime(NOW), DAYS),
        Report::default()
    );
    assert_eq!(
        retention::count_older_than(&store, CaptureTime(NOW), DAYS),
        0
    );
}

#[test]
fn symlinks_are_not_followed_and_targets_survive() {
    let f = fx();
    let outside = f.dir.path().join("outside");
    fs::create_dir(&outside).unwrap();
    let secret = outside.join("secret.txt");
    fs::write(&secret, b"keep me").unwrap();
    let old_in_outside = outside.join(format!("{}.trc", CaptureTime(BOUNDARY - 1).file_stem()));
    fs::write(&old_in_outside, b"keep me too").unwrap();

    // A file symlink with an old capture name, pointing outside.
    let old = CaptureTime(BOUNDARY - 5 * MS_PER_DAY);
    let day_dir = f.store.captures_dir().join(old.day_name());
    fs::create_dir_all(&day_dir).unwrap();
    let link = day_dir.join(format!("{}.trc", old.file_stem()));
    symlink(&secret, &link).unwrap();

    // A day folder symlink pointing at a folder outside with an old file.
    let old_day = CaptureTime(BOUNDARY - 9 * MS_PER_DAY);
    let day_link = f.store.captures_dir().join(old_day.day_name());
    let target_dir = outside.join("days");
    fs::create_dir(&target_dir).unwrap();
    let inside_target = target_dir.join(format!("{}.trc", old_day.file_stem()));
    fs::write(&inside_target, b"keep").unwrap();
    symlink(&target_dir, &day_link).unwrap();

    assert_eq!(f.count(), 0);
    assert_eq!(
        f.run(),
        Report {
            deleted: 0,
            failed: 0
        }
    );
    assert_eq!(fs::read(&secret).unwrap(), b"keep me");
    assert!(old_in_outside.exists());
    assert!(inside_target.exists());
    assert!(fs::symlink_metadata(&link)
        .unwrap()
        .file_type()
        .is_symlink());
    assert!(fs::symlink_metadata(&day_link)
        .unwrap()
        .file_type()
        .is_symlink());
    // The folder with the symlink is not empty and was not removed.
    assert!(day_dir.exists());
}

#[test]
fn unknown_names_are_not_deleted() {
    let f = fx();
    let t = BOUNDARY - 3 * MS_PER_DAY;
    let stem = CaptureTime(t).file_stem();
    let names = [
        "notes.txt".to_string(),
        format!("{stem}.txt"),
        format!("{stem}.trc.bak"),
        format!("{stem}.partial"), // missing the leading dot
        format!(".{stem}.trc"),
        "20250230T120000000Z.trc".to_string(), // date does not exist
        // A valid capture name from another day than its folder.
        format!("{}.trc", CaptureTime(t + 2 * MS_PER_DAY).file_stem()),
    ];
    let paths: Vec<PathBuf> = names.iter().map(|n| f.file_in_day(t, n)).collect();
    // Unknown entries in the captures folder itself and in a non-day folder.
    let stray = f.store.captures_dir().join("stray.trc");
    fs::write(&stray, b"x").unwrap();
    let odd_dir = f.store.captures_dir().join("not-a-day");
    fs::create_dir(&odd_dir).unwrap();
    let in_odd = odd_dir.join(format!("{stem}.trc"));
    fs::write(&in_odd, b"x").unwrap();
    // A subfolder that has a capture name.
    let subdir = f
        .store
        .captures_dir()
        .join(CaptureTime(t).day_name())
        .join(format!("{}.trc", CaptureTime(t - 1).file_stem()));
    fs::create_dir(&subdir).unwrap();

    assert_eq!(f.count(), 0);
    assert_eq!(
        f.run(),
        Report {
            deleted: 0,
            failed: 0
        }
    );
    for p in paths.iter().chain([&stray, &in_odd, &subdir]) {
        assert!(p.exists(), "{} was deleted", p.display());
    }
}

/// Makes `dir` read-only so its files cannot be deleted. Returns false when
/// that has no effect (running as root).
fn lock_dir(dir: &Path) -> bool {
    fs::set_permissions(dir, fs::Permissions::from_mode(0o500)).unwrap();
    let probe = dir.join("probe");
    let writable = fs::write(&probe, b"x").is_ok();
    if writable {
        fs::remove_file(&probe).unwrap();
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700)).unwrap();
    }
    !writable
}

#[test]
fn failed_delete_is_counted_and_retried_after_fixing_permissions() {
    let f = fx();
    let file = f.capture(BOUNDARY - 1);
    fs::set_permissions(&file, fs::Permissions::from_mode(0o400)).unwrap();
    let dir = file.parent().unwrap().to_path_buf();
    if !lock_dir(&dir) {
        eprintln!("skipped: permissions are not enforced (running as root)");
        return;
    }

    assert_eq!(f.count(), 1);
    assert_eq!(
        f.run(),
        Report {
            deleted: 0,
            failed: 1
        }
    );
    assert!(file.exists());

    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(
        f.run(),
        Report {
            deleted: 1,
            failed: 0
        }
    );
    assert!(!file.exists());
    assert!(!dir.exists(), "the empty day folder is removed");
}

#[test]
fn count_matches_what_run_deletes() {
    let f = fx();
    for i in 0..5 {
        f.capture(BOUNDARY - 1 - i * 3_600_000);
    }
    f.partial(BOUNDARY - 2 * MS_PER_DAY);
    f.capture(BOUNDARY);
    f.capture(NOW);
    f.file_in_day(BOUNDARY - 1, "unknown.dat");

    let before = f.store.list(..).unwrap().len();
    let counted = f.count();
    // Counting deletes nothing.
    assert_eq!(f.store.list(..).unwrap().len(), before);
    let report = f.run();
    assert_eq!(counted, 6);
    assert_eq!(
        report,
        Report {
            deleted: counted,
            failed: 0
        }
    );
    assert_eq!(f.store.list(..).unwrap().len(), before - 5);
    assert_eq!(f.count(), 0);
    assert_eq!(f.run(), Report::default());
}

#[test]
fn empty_day_folders_are_removed() {
    let f = fx();
    let emptied = f.capture(BOUNDARY - 1).parent().unwrap().to_path_buf();
    let already_empty = f
        .store
        .captures_dir()
        .join(CaptureTime(BOUNDARY - 10 * MS_PER_DAY).day_name());
    fs::create_dir(&already_empty).unwrap();
    let kept_day = f.capture(NOW).parent().unwrap().to_path_buf();
    let with_unknown = f.file_in_day(BOUNDARY - 20 * MS_PER_DAY, "unknown.dat");

    assert_eq!(
        f.run(),
        Report {
            deleted: 1,
            failed: 0
        }
    );
    assert!(!emptied.exists());
    assert!(!already_empty.exists());
    assert!(kept_day.exists());
    assert!(with_unknown.exists());
    assert!(f.store.captures_dir().exists());
}

#[test]
fn stored_captures_are_deleted_by_name_without_a_key() {
    let f = fx();
    let id = age::x25519::Identity::generate();
    let pk = id.to_public().to_string();
    let meta = |ms| crate::CaptureMeta {
        taken_at_ms: ms,
        tz_offset_min: 0,
        app_name: "App".into(),
        window_title: "Title".into(),
        width: 1,
        height: 1,
        format: "webp".into(),
    };
    f.store.write(&pk, &meta(BOUNDARY - 1), b"t", b"i").unwrap();
    f.store.write(&pk, &meta(BOUNDARY), b"t", b"i").unwrap();
    assert_eq!(f.run().deleted, 1);
    assert_eq!(f.store.list(..).unwrap(), vec![CaptureTime(BOUNDARY)]);
    assert!(f.store.read_meta(&id, CaptureTime(BOUNDARY)).is_ok());
}
