//! The restore token is the only thing the Wayland source writes: the file is
//! mode 0600 and no frame data reaches TMPDIR or the state directory.
//!
//! This is the only test in this binary, because it sets `TMPDIR`.
#![allow(clippy::unwrap_used)]

use std::collections::BTreeSet;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::Duration;

use notify::{EventKind, RecursiveMode, Watcher};
use timereel_capture::wayland::fake::{FakePortal, FakeStream};
use timereel_capture::wayland::TokenStore;
use timereel_capture::{ScreenSource, WaylandScreen};

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        out.push(p.clone());
        if p.is_dir() {
            walk(&p, out);
        }
    }
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

#[test]
fn the_token_file_is_0600_and_no_frame_data_is_written() {
    let root = tempfile::tempdir().unwrap();
    let tmp = root.path().join("tmp");
    let state = root.path().join("state");
    std::fs::create_dir_all(&tmp).unwrap();
    std::fs::create_dir_all(&state).unwrap();
    std::env::set_var("TMPDIR", &tmp);
    std::env::set_var("TIMEREEL_STATE_DIR", &state);
    assert_eq!(std::env::temp_dir(), tmp);

    let (tx, rx) = mpsc::channel();
    let mut watcher = notify::recommended_watcher(move |ev| {
        let _ = tx.send(ev);
    })
    .unwrap();
    watcher.watch(&tmp, RecursiveMode::Recursive).unwrap();
    watcher.watch(&state, RecursiveMode::Recursive).unwrap();

    // A frame with a recognisable colour: a distinctive BGRx pixel.
    let marker = [0x12u8, 0x34, 0x56, 0x00];
    let mut stream = FakeStream::solid(1, (0, 0), 64, 64, [0x56, 0x34, 0x12]);
    stream.data = marker.repeat(64 * 64);
    let portal = Arc::new(FakePortal::with_streams(5, vec![stream]));
    let tokens = TokenStore::at_default_location().unwrap();
    assert_eq!(tokens.path(), state.join("portal-restore-token"));
    let source = WaylandScreen::new(Arc::clone(&portal), tokens);
    for _ in 0..3 {
        assert_eq!(source.capture().unwrap().len(), 1);
    }

    // The token file: 0600, in a 0700 directory.
    let token_path = state.join("portal-restore-token");
    let mode = std::fs::metadata(&token_path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
    let dir_mode = std::fs::metadata(&state).unwrap().permissions().mode() & 0o777;
    assert_eq!(dir_mode, 0o700);
    assert_eq!(std::fs::read_to_string(&token_path).unwrap(), "token-3");

    // Nothing else exists in either directory, and nothing holds pixels.
    let mut files = Vec::new();
    walk(&tmp, &mut files);
    walk(&state, &mut files);
    assert_eq!(files, vec![token_path.clone()], "only the token file");
    let bytes = std::fs::read(&token_path).unwrap();
    assert!(!contains(&bytes, &marker[..3]));

    // Every file the watcher saw was the token or its temp file, and no event
    // came from TMPDIR at all.
    std::thread::sleep(Duration::from_millis(200));
    let mut seen = BTreeSet::new();
    while let Ok(ev) = rx.try_recv() {
        let ev = ev.unwrap();
        if matches!(ev.kind, EventKind::Access(_)) {
            continue;
        }
        for p in ev.paths {
            assert!(!p.starts_with(&tmp), "wrote into TMPDIR: {p:?}");
            seen.insert(p.file_name().unwrap().to_string_lossy().into_owned());
        }
    }
    let allowed = BTreeSet::from([
        "portal-restore-token".to_string(),
        ".portal-restore-token.partial".to_string(),
        // The state directory itself (its mode or times change).
        "state".to_string(),
    ]);
    assert!(seen.is_subset(&allowed), "unexpected files: {seen:?}");
    assert!(seen.contains("portal-restore-token"));
}
