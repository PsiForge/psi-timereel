//! A tampered capture gives an error item, and the loader keeps working.

#![allow(clippy::unwrap_used)]

use std::fs;
use std::sync::Arc;
use std::time::{Duration, Instant};

use timereel_image::{encode, Format, RgbaImage};
use timereel_store::{CaptureMeta, CaptureTime, Store};
use timereel_timeline::{ImageLoader, Item, Kind, Loaded, StoreDecoder};

fn picture(w: u32, h: u32) -> RgbaImage {
    let data = (0..w * h * 4).map(|i| (i % 253) as u8).collect();
    RgbaImage::new(w, h, data).unwrap()
}

#[test]
fn a_tampered_capture_produces_an_error_item_and_the_loader_keeps_working() {
    let dir = tempfile::tempdir().unwrap();
    let identity = age::x25519::Identity::generate();
    let pk = identity.to_public().to_string();
    let store = Store::new(dir.path());
    let thumb = encode(&picture(24, 16), Format::Webp, 40).unwrap();
    let image = encode(&picture(96, 64), Format::Webp, 50).unwrap();
    let times: Vec<CaptureTime> = (0..3)
        .map(|i| CaptureTime(1_747_215_000_000 + i * 300_000))
        .collect();
    for t in &times {
        let meta = CaptureMeta {
            taken_at_ms: t.0,
            tz_offset_min: 0,
            app_name: "app".into(),
            window_title: "title".into(),
            width: 96,
            height: 64,
            format: "webp".into(),
        };
        store.write(&pk, &meta, &thumb, &image).unwrap();
    }

    // Flip one bit in the last byte of the middle file: the image part.
    let path = store.path_of(times[1]);
    let mut bytes = fs::read(&path).unwrap();
    *bytes.last_mut().unwrap() ^= 1;
    fs::write(&path, bytes).unwrap();

    let loader = ImageLoader::new(Arc::new(StoreDecoder::new(store, identity)), || {});
    for t in &times {
        loader.request(Kind::Full, *t);
    }
    loader.request(Kind::Thumb, times[1]);
    let mut got: Vec<Loaded> = Vec::new();
    let end = Instant::now() + Duration::from_secs(20);
    while got.len() < 4 {
        assert!(Instant::now() < end, "timed out");
        got.extend(loader.poll());
        std::thread::sleep(Duration::from_millis(5));
    }
    let find = |kind, t: CaptureTime| got.iter().find(|r| r.kind == kind && r.time == t).unwrap();
    assert!(matches!(find(Kind::Full, times[1]).item, Item::Failed(_)));
    for t in [times[0], times[2]] {
        match &find(Kind::Full, t).item {
            Item::Ready(img) => assert_eq!((img.width(), img.height()), (96, 64)),
            other => panic!("expected an image, got {other:?}"),
        }
    }
    // The untouched thumbnail of the tampered file still loads.
    assert!(matches!(find(Kind::Thumb, times[1]).item, Item::Ready(_)));
}
