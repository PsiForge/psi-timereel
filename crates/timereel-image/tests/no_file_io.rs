//! The image pipeline must not touch the file system. This runs the whole API
//! with `TMPDIR` pointing at a watched directory and expects no event at all.
//! It lives in its own test binary because it changes the process environment.

#![allow(clippy::unwrap_used)]

use notify::{RecursiveMode, Watcher};
use std::sync::mpsc;
use std::time::Duration;
use timereel_image::{decode, encode, stitch, thumbnail, Format, Frame, RgbaImage};

#[test]
fn pipeline_creates_no_files_in_tmpdir() {
    let tmp = tempfile::tempdir().unwrap();
    let watched = tmp.path().canonicalize().unwrap();
    std::env::set_var("TMPDIR", &watched);
    std::env::set_var("TMP", &watched);
    std::env::set_var("TEMP", &watched);

    let (tx, rx) = mpsc::channel();
    let mut watcher = notify::recommended_watcher(tx).unwrap();
    watcher.watch(&watched, RecursiveMode::Recursive).unwrap();

    let shot = |w: u32, h: u32| {
        let data = (0..w * h)
            .flat_map(|i| [(i % 251) as u8, (i / w % 241) as u8, 90, 255])
            .collect();
        Frame::new(0, 0, RgbaImage::new(w, h, data).unwrap())
    };
    let mut right = shot(1920, 1080);
    right.x = 1920;
    let stitched = stitch(&[shot(1920, 1080), right], 1280).unwrap();
    let thumb = thumbnail(&stitched, 240).unwrap();
    for format in [Format::Webp, Format::Jpeg] {
        for img in [&stitched, &thumb] {
            let bytes = encode(img, format, 50).unwrap();
            let back = decode(&bytes).unwrap();
            assert_eq!(back.width(), img.width());
        }
    }

    // Give inotify time to report anything that happened.
    std::thread::sleep(Duration::from_millis(300));
    let events: Vec<_> = rx.try_iter().collect();
    assert!(
        events.is_empty(),
        "file system activity in TMPDIR: {events:?}"
    );
    assert_eq!(std::fs::read_dir(&watched).unwrap().count(), 0);

    // Sanity check: the watcher really does see a new file.
    std::fs::write(watched.join("probe"), b"x").unwrap();
    let event = rx.recv_timeout(Duration::from_secs(5));
    assert!(event.is_ok(), "the watcher did not report a probe file");
}

#[test]
fn library_source_has_no_file_api() {
    let src = include_str!("../src/lib.rs");
    for forbidden in [
        "std::fs",
        "File::",
        "OpenOptions",
        "std::io::Write",
        "tempfile",
        "TMPDIR",
    ] {
        assert!(!src.contains(forbidden), "lib.rs mentions `{forbidden}`");
    }
}
