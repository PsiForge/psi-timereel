//! While captures are written, a file watcher on the data directory and on
//! TMPDIR sees only `.partial` and `.trc` files, and none of them holds
//! plaintext. This is a separate test binary so that it can own TMPDIR.

#![allow(clippy::unwrap_used)]

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

use notify::{EventKind, RecursiveMode, Watcher};
use timereel_store::{CaptureMeta, Store};

const N: i64 = 50;
const SECRET_APP: &str = "SecretAppName";
const SECRET_TITLE: &str = "Secret window title: bank statement";

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

fn is_day_folder(name: &str) -> bool {
    name.len() == 10 && name.as_bytes()[4] == b'-' && name.as_bytes()[7] == b'-'
}

/// A name the store may create: a day folder, `<time>.trc` or `.<time>.partial`.
fn allowed(path: &Path, data: &Path) -> bool {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    path == data
        || name == "captures"
        || is_day_folder(name)
        || name.ends_with(".trc")
        || (name.starts_with('.') && name.ends_with(".partial"))
}

fn walk(dir: &Path, files: &mut Vec<PathBuf>) {
    for e in fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
        if p.is_dir() {
            walk(&p, files);
        } else {
            files.push(p);
        }
    }
}

#[test]
fn fifty_writes_touch_only_partial_and_trc_files_with_no_plaintext() {
    let base = tempfile::tempdir().unwrap();
    let data = base.path().join("data");
    let tmp = base.path().join("tmp");
    fs::create_dir_all(&data).unwrap();
    fs::create_dir_all(&tmp).unwrap();
    std::env::set_var("TMPDIR", &tmp);
    assert_eq!(std::env::temp_dir(), tmp);

    let (tx, rx) = mpsc::channel();
    let mut watcher = notify::recommended_watcher(move |ev| {
        let _ = tx.send(ev);
    })
    .unwrap();
    watcher.watch(&data, RecursiveMode::Recursive).unwrap();
    watcher.watch(&tmp, RecursiveMode::Recursive).unwrap();

    let identity = age::x25519::Identity::generate();
    let pk = identity.to_public().to_string();
    let store = Store::new(&data);
    // A fake WebP image: it must not appear in the file as it is.
    let mut image = b"RIFF\x10\x00\x00\x00WEBPVP8 ".to_vec();
    image.extend((0..4000u32).map(|i| (i % 251) as u8));
    let thumb = [b"RIFF\x08\x00\x00\x00WEBPVP8L".as_slice(), &[9u8; 300]].concat();
    // Spread over three UTC days.
    for i in 0..N {
        let meta = CaptureMeta {
            taken_at_ms: 1_747_215_000_000 + i * 1_700_000,
            tz_offset_min: 60,
            app_name: SECRET_APP.into(),
            window_title: SECRET_TITLE.into(),
            width: 1280,
            height: 720,
            format: "webp".into(),
        };
        store.write(&pk, &meta, &thumb, &image).unwrap();
    }
    assert_eq!(store.list(..).unwrap().len(), N as usize);

    // Let the watcher deliver the last events.
    std::thread::sleep(Duration::from_millis(500));
    let mut seen = BTreeSet::new();
    while let Ok(ev) = rx.try_recv() {
        let ev = ev.unwrap();
        if matches!(ev.kind, EventKind::Access(_)) {
            continue;
        }
        for p in ev.paths {
            assert!(
                allowed(&p, &data),
                "unexpected file touched: {}",
                p.display()
            );
            assert!(!p.starts_with(&tmp), "file in TMPDIR: {}", p.display());
            seen.insert(p);
        }
    }
    assert!(
        seen.iter()
            .any(|p| p.to_string_lossy().ends_with(".partial")),
        "the watcher saw no partial file"
    );
    assert!(seen.iter().any(|p| p.to_string_lossy().ends_with(".trc")));

    // Nothing in TMPDIR; only finished .trc files remain in the data dir.
    assert_eq!(fs::read_dir(&tmp).unwrap().count(), 0);
    let mut files = Vec::new();
    walk(&data, &mut files);
    assert_eq!(files.len(), N as usize);
    for f in files {
        assert_eq!(f.extension().and_then(|e| e.to_str()), Some("trc"));
        let bytes = fs::read(&f).unwrap();
        assert_eq!(&bytes[..4], b"TRC1");
        for needle in [
            b"RIFF".as_slice(),
            b"WEBP",
            SECRET_APP.as_bytes(),
            SECRET_TITLE.as_bytes(),
            b"taken_at_ms",
            b"window_title",
            b"app_name",
        ] {
            assert!(
                !contains(&bytes, needle),
                "{} contains {:?}",
                f.display(),
                String::from_utf8_lossy(needle)
            );
        }
    }
}
