//! Loading the metadata and searching it write nothing to disk. This is its
//! own test binary so that it can own TMPDIR, XDG_CACHE_HOME and
//! XDG_CONFIG_HOME.

#![allow(clippy::unwrap_used)]

use std::fs;
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

use notify::{EventKind, RecursiveMode, Watcher};
use timereel_image::{encode, Format, RgbaImage};
use timereel_store::{CaptureMeta, Store};
use timereel_timeline::{Index, MetaStore, SearchFilter, StoreMetaSource};

#[test]
fn nothing_is_written_to_disk_during_a_search() {
    let base = tempfile::tempdir().unwrap();
    let (data, config, tmp, cache) = (
        base.path().join("data"),
        base.path().join("config"),
        base.path().join("tmp"),
        base.path().join("cache"),
    );
    for d in [&data, &config, &tmp, &cache] {
        fs::create_dir_all(d).unwrap();
    }
    std::env::set_var("TMPDIR", &tmp);
    std::env::set_var("XDG_CACHE_HOME", &cache);
    std::env::set_var("XDG_CONFIG_HOME", &config);
    assert_eq!(std::env::temp_dir(), tmp);

    let identity = age::x25519::Identity::generate();
    let pk = identity.to_public().to_string();
    let store = Store::new(&data);
    let px = RgbaImage::new(32, 24, vec![90; 32 * 24 * 4]).unwrap();
    let thumb = encode(&px, Format::Webp, 40).unwrap();
    let image = encode(&px, Format::Webp, 50).unwrap();
    for i in 0..12 {
        let meta = CaptureMeta {
            taken_at_ms: 1_747_215_000_000 + i * 300_000,
            tz_offset_min: 0,
            app_name: if i % 3 == 0 { "Editor" } else { "Browser" }.into(),
            window_title: format!("Ärger number {i}"),
            width: 32,
            height: 24,
            format: "webp".into(),
        };
        store.write(&pk, &meta, &thumb, &image).unwrap();
    }
    let index = Index::load(&store).unwrap();
    assert_eq!(index.len(), 12);

    // Watch only after the set-up, so that only the search is observed.
    let (tx, rx) = mpsc::channel();
    let mut watcher = notify::recommended_watcher(move |ev| {
        let _ = tx.send(ev);
    })
    .unwrap();
    for d in [&data, &config, &tmp, &cache] {
        watcher.watch(d, RecursiveMode::Recursive).unwrap();
    }

    let meta = MetaStore::new(Arc::new(StoreMetaSource::new(store, identity)), || {});
    meta.queue(index.times().iter().rev().copied().collect());
    let end = Instant::now() + Duration::from_secs(20);
    while !meta.is_complete() {
        assert!(Instant::now() < end, "timed out");
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(meta.len(), 12);

    for query in ["editor", "ÄRGER", "number 7", "nothing like this", ""] {
        let filter = SearchFilter::new(query);
        let found = meta.matching(index.times(), &filter);
        let expect = match query {
            "editor" => 4,
            "ÄRGER" | "" => 12,
            "number 7" => 1,
            _ => 0,
        };
        assert_eq!(found.len(), expect, "query {query}");
    }
    let filter = SearchFilter::new("editor");
    let first = index.first().unwrap();
    assert!(index
        .next_matching(first, |t| meta.matches(t, &filter))
        .is_some());

    (meta.lock_hook())();
    drop(meta);
    std::thread::sleep(Duration::from_millis(500));

    // Reads show up as access events only; anything else is a write.
    while let Ok(ev) = rx.try_recv() {
        let ev = ev.unwrap();
        assert!(
            matches!(ev.kind, EventKind::Access(_)),
            "disk was changed during the search: {ev:?}"
        );
    }
    for d in [&config, &tmp, &cache] {
        assert_eq!(fs::read_dir(d).unwrap().count(), 0, "{d:?} is not empty");
    }
}
