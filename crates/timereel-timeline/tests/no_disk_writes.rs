//! Loading writes nothing to disk. This is its own test binary so that it
//! can own TMPDIR and XDG_CACHE_HOME.

#![allow(clippy::unwrap_used)]

use std::fs;
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

use notify::{EventKind, RecursiveMode, Watcher};
use timereel_image::{encode, Format, RgbaImage};
use timereel_store::{CaptureMeta, CaptureTime, Store};
use timereel_timeline::{ImageLoader, Index, StoreDecoder};

#[test]
fn nothing_is_written_to_disk_during_loading() {
    let base = tempfile::tempdir().unwrap();
    let (data, tmp, cache) = (
        base.path().join("data"),
        base.path().join("tmp"),
        base.path().join("cache"),
    );
    for d in [&data, &tmp, &cache] {
        fs::create_dir_all(d).unwrap();
    }
    std::env::set_var("TMPDIR", &tmp);
    std::env::set_var("XDG_CACHE_HOME", &cache);
    assert_eq!(std::env::temp_dir(), tmp);

    let identity = age::x25519::Identity::generate();
    let pk = identity.to_public().to_string();
    let store = Store::new(&data);
    let px = RgbaImage::new(64, 48, vec![200; 64 * 48 * 4]).unwrap();
    let thumb = encode(&px, Format::Webp, 40).unwrap();
    let image = encode(&px, Format::Webp, 50).unwrap();
    for i in 0..10 {
        let meta = CaptureMeta {
            taken_at_ms: 1_747_215_000_000 + i * 300_000,
            tz_offset_min: 0,
            app_name: "app".into(),
            window_title: "title".into(),
            width: 64,
            height: 48,
            format: "webp".into(),
        };
        store.write(&pk, &meta, &thumb, &image).unwrap();
    }
    let index = Index::load(&store).unwrap();
    assert_eq!(index.len(), 10);

    // Watch only after the set-up, so that only loading is observed.
    let (tx, rx) = mpsc::channel();
    let mut watcher = notify::recommended_watcher(move |ev| {
        let _ = tx.send(ev);
    })
    .unwrap();
    for d in [&data, &tmp, &cache] {
        watcher.watch(d, RecursiveMode::Recursive).unwrap();
    }

    let loader = ImageLoader::new(Arc::new(StoreDecoder::new(store, identity)), || {});
    loader.prefetch(&index, CaptureTime(1_747_215_000_000 + 4 * 300_000));
    let mut delivered = 0;
    let end = Instant::now() + Duration::from_secs(20);
    while delivered < 7 + 10 {
        assert!(Instant::now() < end, "timed out");
        delivered += loader.poll().len();
        std::thread::sleep(Duration::from_millis(5));
    }
    loader.clear();
    drop(loader);
    std::thread::sleep(Duration::from_millis(500));

    // Reads show up as access events only; anything else is a write.
    while let Ok(ev) = rx.try_recv() {
        let ev = ev.unwrap();
        assert!(
            matches!(ev.kind, EventKind::Access(_)),
            "disk was changed during loading: {ev:?}"
        );
    }
    assert_eq!(fs::read_dir(&tmp).unwrap().count(), 0);
    assert_eq!(fs::read_dir(&cache).unwrap().count(), 0);
}
