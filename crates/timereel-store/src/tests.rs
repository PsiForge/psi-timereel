#![allow(clippy::unwrap_used)]

use std::fs;
use std::os::unix::fs::PermissionsExt;

use age::x25519::Identity;

use crate::time::{parse_day_name, CaptureTime};
use crate::*;

fn meta_at(ms: i64) -> CaptureMeta {
    CaptureMeta {
        taken_at_ms: ms,
        tz_offset_min: 120,
        app_name: "Firefox".into(),
        window_title: "Inbox".into(),
        width: 1280,
        height: 720,
        format: "webp".into(),
    }
}

struct Fixture {
    _dir: tempfile::TempDir,
    store: Store,
    id: Identity,
    pk: String,
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let id = Identity::generate();
    Fixture {
        store: Store::new(dir.path()),
        pk: id.to_public().to_string(),
        id,
        _dir: dir,
    }
}

// 2025-05-14T09:30:00.123Z
const T0: i64 = 1_747_215_000_123;
const HOUR: i64 = 3_600_000;

#[test]
fn time_names_round_trip() {
    let t = CaptureTime(T0);
    assert_eq!(t.file_stem(), "20250514T093000123Z");
    assert_eq!(t.day_name(), "2025-05-14");
    assert_eq!(CaptureTime::parse_stem("20250514T093000123Z"), Some(t));
    assert_eq!(CaptureTime(0).file_stem(), "19700101T000000000Z");
    assert_eq!(CaptureTime(-1).file_stem(), "19691231T235959999Z");
    assert_eq!(CaptureTime(-1).day_name(), "1969-12-31");
    // Leap day, and dates that do not exist.
    let leap = CaptureTime::parse_stem("20240229T120000000Z").unwrap();
    assert_eq!(leap.file_stem(), "20240229T120000000Z");
    for bad in [
        "20250230T120000000Z",
        "20250514T240000000Z",
        "20250514T093000123",
        "2025-05-14T093000123Z",
        "20250514T09300012aZ",
        "20231301T000000000Z",
        "",
    ] {
        assert_eq!(CaptureTime::parse_stem(bad), None, "{bad}");
    }
    assert_eq!(parse_day_name("2025-05-14"), Some(CaptureTime(T0).day()));
    assert_eq!(parse_day_name("2025-02-30"), None);
    assert_eq!(parse_day_name("2025-5-14"), None);
    assert_eq!(parse_day_name("+025-05-14"), None);
}

#[test]
fn write_then_read_round_trips() {
    let f = fixture();
    let thumb = b"thumb bytes".to_vec();
    let image = vec![7u8; 50_000];
    let t = f.store.write(&f.pk, &meta_at(T0), &thumb, &image).unwrap();
    assert_eq!(t, CaptureTime(T0));
    assert_eq!(
        f.store
            .path_of(t)
            .strip_prefix(f.store.captures_dir())
            .unwrap(),
        std::path::Path::new("2025-05-14/20250514T093000123Z.trc")
    );
    assert_eq!(f.store.read_meta(&f.id, t).unwrap(), meta_at(T0));
    assert_eq!(f.store.read_thumb(&f.id, t).unwrap(), thumb);
    assert_eq!(f.store.read_image(&f.id, t).unwrap(), image);
}

#[test]
fn reading_a_missing_capture_is_not_found() {
    let f = fixture();
    let e = f.store.read_meta(&f.id, CaptureTime(T0)).unwrap_err();
    assert!(matches!(e, StoreError::NotFound(_)), "{e}");
}

#[test]
fn wrong_key_is_an_error() {
    let f = fixture();
    let t = f.store.write(&f.pk, &meta_at(T0), b"t", b"i").unwrap();
    let other = Identity::generate();
    assert!(matches!(
        f.store.read_meta(&other, t),
        Err(StoreError::Container(_))
    ));
}

#[test]
fn an_existing_capture_is_not_overwritten() {
    let f = fixture();
    f.store.write(&f.pk, &meta_at(T0), b"t", b"first").unwrap();
    let e = f
        .store
        .write(&f.pk, &meta_at(T0), b"t", b"second")
        .unwrap_err();
    assert!(matches!(e, StoreError::AlreadyExists(_)), "{e}");
    assert_eq!(
        f.store.read_image(&f.id, CaptureTime(T0)).unwrap(),
        b"first"
    );
    // The failed attempt left no partial file behind.
    assert_eq!(f.store.cleanup_partials().unwrap(), 0);
}

#[test]
fn out_of_range_time_is_refused() {
    let f = fixture();
    let e = f
        .store
        .write(&f.pk, &meta_at(i64::MAX), b"t", b"i")
        .unwrap_err();
    assert!(matches!(e, StoreError::TimeOutOfRange(_)), "{e}");
}

#[test]
fn bad_public_key_is_a_container_error_and_writes_nothing() {
    let f = fixture();
    let e = f
        .store
        .write("not a key", &meta_at(T0), b"t", b"i")
        .unwrap_err();
    assert!(matches!(e, StoreError::Container(_)), "{e}");
    assert!(!f.store.captures_dir().exists());
}

#[test]
fn list_is_sorted_across_days_and_filters_by_range() {
    let f = fixture();
    // Written out of order, over three UTC days (one is across midnight).
    let times = [
        T0 + 30 * HOUR,
        T0,
        T0 + HOUR,
        T0 + 15 * HOUR, // 2025-05-15 00:30:00.123
        T0 - 24 * HOUR,
    ];
    for t in times {
        f.store.write(&f.pk, &meta_at(t), b"t", b"i").unwrap();
    }
    let mut sorted: Vec<_> = times.iter().map(|t| CaptureTime(*t)).collect();
    sorted.sort();
    assert_eq!(f.store.list(..).unwrap(), sorted);

    let range = f
        .store
        .list(CaptureTime(T0)..CaptureTime(T0 + 15 * HOUR))
        .unwrap();
    assert_eq!(range, [CaptureTime(T0), CaptureTime(T0 + HOUR)]);
    let incl = f
        .store
        .list(CaptureTime(T0)..=CaptureTime(T0 + 15 * HOUR))
        .unwrap();
    assert_eq!(incl.len(), 3);
    assert_eq!(f.store.list(CaptureTime(T0 + HOUR + 1)..).unwrap().len(), 2);
    assert_eq!(
        f.store.list(..CaptureTime(T0)).unwrap(),
        [CaptureTime(T0 - 24 * HOUR)]
    );
    assert!(f
        .store
        .list(CaptureTime(T0 + 100 * HOUR)..)
        .unwrap()
        .is_empty());
}

#[test]
fn list_of_a_missing_store_is_empty() {
    let f = fixture();
    assert!(f.store.list(..).unwrap().is_empty());
    assert_eq!(f.store.cleanup_partials().unwrap(), 0);
}

#[test]
fn list_ignores_partial_and_unknown_names() {
    let f = fixture();
    let t = f.store.write(&f.pk, &meta_at(T0), b"t", b"i").unwrap();
    let day = f.store.path_of(t).parent().unwrap().to_path_buf();
    for name in [
        ".20250514T100000000Z.partial",
        "20250514T100000000Z.partial",
        "20250514T100000000Z.trc.tmp",
        "20250514T100000000Z.TRC",
        "notes.txt",
        "20250515T000000000Z.trc", // right shape, wrong day folder
        "20250230T000000000Z.trc", // no such date
    ] {
        fs::write(day.join(name), b"x").unwrap();
    }
    fs::create_dir(day.join("20250514T110000000Z.trc")).unwrap(); // a directory
    fs::create_dir(f.store.captures_dir().join("lost+found")).unwrap();
    fs::write(f.store.captures_dir().join("2025-05-16"), b"file").unwrap();
    std::os::unix::fs::symlink(f.store.path_of(t), day.join("20250514T120000000Z.trc")).unwrap();
    assert_eq!(f.store.list(..).unwrap(), [t]);
}

#[test]
fn interrupted_write_is_not_listed_and_cleanup_removes_it() {
    let f = fixture();
    let done = f.store.write(&f.pk, &meta_at(T0), b"t", b"i").unwrap();
    // The write stops before the rename.
    let staged = f
        .store
        .stage(&f.pk, &meta_at(T0 + HOUR), b"t", b"i")
        .unwrap();
    assert!(staged.partial.exists());
    assert!(!staged.final_path.exists());
    assert_eq!(f.store.list(..).unwrap(), [done]);
    assert!(matches!(
        f.store.read_meta(&f.id, CaptureTime(T0 + HOUR)),
        Err(StoreError::NotFound(_))
    ));

    assert_eq!(f.store.cleanup_partials().unwrap(), 1);
    assert!(!staged.partial.exists());
    assert_eq!(f.store.list(..).unwrap(), [done]);
    assert!(f.store.read_meta(&f.id, done).is_ok());
    assert_eq!(f.store.cleanup_partials().unwrap(), 0);
}

#[test]
fn staged_write_can_be_committed() {
    let f = fixture();
    let staged = f.store.stage(&f.pk, &meta_at(T0), b"t", b"i").unwrap();
    f.store.commit(&staged).unwrap();
    assert_eq!(f.store.list(..).unwrap(), [CaptureTime(T0)]);
}

#[test]
fn renamed_file_gives_a_clear_error() {
    let f = fixture();
    let t = f
        .store
        .write(&f.pk, &meta_at(T0), b"thumb", b"image")
        .unwrap();
    let new = CaptureTime(T0 + 5 * 60_000);
    let new_path = f.store.path_of(new);
    fs::rename(f.store.path_of(t), &new_path).unwrap();
    assert_eq!(f.store.list(..).unwrap(), [new]);
    for result in [
        f.store.read_meta(&f.id, new).map(drop),
        f.store.read_thumb(&f.id, new).map(drop),
        f.store.read_image(&f.id, new).map(drop),
    ] {
        let e = result.unwrap_err();
        assert!(
            matches!(
                e,
                StoreError::TimestampMismatch { file, inside }
                    if file == new && inside == t
            ),
            "{e}"
        );
        let msg = e.to_string();
        assert!(
            msg.contains("renamed") && msg.contains("09:35:00.123"),
            "{msg}"
        );
    }
}

#[test]
fn parts_swapped_between_captures_are_detected() {
    let f = fixture();
    let a = f
        .store
        .write(&f.pk, &meta_at(T0), b"thumb-a", b"image-a")
        .unwrap();
    let b = f
        .store
        .write(&f.pk, &meta_at(T0 + 1000), b"thumb-b", b"image-b")
        .unwrap();
    // Replace the whole thumb part of A with the thumb part of B.
    let mut bytes_a = fs::read(f.store.path_of(a)).unwrap();
    let bytes_b = fs::read(f.store.path_of(b)).unwrap();
    let header = timereel_crypto::Header::parse(&bytes_a).unwrap();
    let hb = timereel_crypto::Header::parse(&bytes_b).unwrap();
    let mut out = bytes_a[..timereel_crypto::HEADER_LEN].to_vec();
    out[5..9].copy_from_slice(&hb.len(timereel_crypto::Part::Thumb).to_le_bytes());
    let range = |h: &timereel_crypto::Header, p| {
        h.offset(p) as usize..(h.offset(p) + u64::from(h.len(p))) as usize
    };
    use timereel_crypto::Part;
    out.extend_from_slice(&bytes_b[range(&hb, Part::Thumb)]);
    out.extend_from_slice(&bytes_a[range(&header, Part::Meta)]);
    out.extend_from_slice(&bytes_a[range(&header, Part::Image)]);
    bytes_a = out;
    fs::write(f.store.path_of(a), bytes_a).unwrap();
    assert!(f.store.read_meta(&f.id, a).is_ok());
    assert!(matches!(
        f.store.read_thumb(&f.id, a),
        Err(StoreError::Container(_))
    ));
    assert_eq!(f.store.read_image(&f.id, a).unwrap(), b"image-a");
}

#[test]
fn damaged_file_is_an_error_not_a_panic() {
    let f = fixture();
    let t = f.store.write(&f.pk, &meta_at(T0), b"t", b"i").unwrap();
    let path = f.store.path_of(t);
    let bytes = fs::read(&path).unwrap();
    fs::write(&path, &bytes[..bytes.len() - 3]).unwrap();
    assert!(f.store.read_image(&f.id, t).is_err());
    assert!(f.store.read_meta(&f.id, t).is_ok());
    fs::write(&path, b"").unwrap();
    assert!(f.store.read_meta(&f.id, t).is_err());
}

#[test]
fn folders_are_0700_and_files_0600() {
    let f = fixture();
    let t = f.store.write(&f.pk, &meta_at(T0), b"t", b"i").unwrap();
    let staged = f
        .store
        .stage(&f.pk, &meta_at(T0 + 1000), b"t", b"i")
        .unwrap();
    let mode = |p: &std::path::Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(f.store.captures_dir()), 0o700);
    assert_eq!(mode(f.store.path_of(t).parent().unwrap()), 0o700);
    assert_eq!(mode(&f.store.path_of(t)), 0o600);
    assert_eq!(mode(&staged.partial), 0o600);
}

#[test]
fn a_loose_existing_captures_folder_is_tightened() {
    let f = fixture();
    fs::create_dir_all(f.store.captures_dir()).unwrap();
    fs::set_permissions(f.store.captures_dir(), fs::Permissions::from_mode(0o755)).unwrap();
    f.store.write(&f.pk, &meta_at(T0), b"t", b"i").unwrap();
    let mode = fs::metadata(f.store.captures_dir())
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o700);
}
