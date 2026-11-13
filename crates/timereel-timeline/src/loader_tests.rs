//! Tests of the byte-limited cache and of the loader with fake decoders.

#![allow(clippy::unwrap_used)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use timereel_image::RgbaImage;

use crate::*;

fn img(bytes: usize) -> Arc<RgbaImage> {
    let px = bytes / 4;
    Arc::new(RgbaImage::new(px as u32, 1, vec![7; px * 4]).unwrap())
}

#[test]
fn eviction_keeps_the_limit_and_removes_the_least_recently_used_first() {
    let mut lru: ByteLru<u32, Arc<RgbaImage>> = ByteLru::new(100);
    assert!(lru.insert(1, img(40)));
    assert!(lru.insert(2, img(40)));
    // Use 1, so that 2 is now the least recently used.
    assert!(lru.get(&1).is_some());
    assert!(lru.insert(3, img(40)));
    assert!(lru.bytes() <= 100);
    assert!(lru.contains(&1), "recently used entry must stay");
    assert!(!lru.contains(&2), "least recently used entry must go first");
    assert!(lru.contains(&3));

    // A big insert evicts several entries, oldest first, and stays in limit.
    assert!(lru.insert(4, img(92)));
    assert_eq!(lru.len(), 1);
    assert!(lru.contains(&4));
    assert_eq!(lru.bytes(), 92);

    // Replacing a key does not double count; too-big values are refused.
    assert!(lru.insert(4, img(60)));
    assert_eq!(lru.bytes(), 60);
    assert!(!lru.insert(5, img(104)));
    assert_eq!(lru.bytes(), 60);
    for i in 0..200 {
        lru.insert(i, img(24));
        assert!(lru.bytes() <= lru.limit());
    }
    lru.clear();
    assert_eq!((lru.bytes(), lru.len()), (0, 0));
}

#[test]
fn standard_limits_are_256_and_64_megabytes() {
    assert_eq!(FULL_CACHE_BYTES, 256 << 20);
    assert_eq!(THUMB_CACHE_BYTES, 64 << 20);
    assert_eq!(PREFETCH_FULL, 3);
    assert_eq!(PREFETCH_THUMBS, 40);
    assert!((1..=4).contains(&default_workers()));
}

/// A decoder that records the order of the jobs. While the gate is closed,
/// every `load` waits, so the test can queue jobs behind a busy worker.
struct Fake {
    calls: Mutex<Vec<(Kind, i64)>>,
    gate: (Mutex<bool>, Condvar),
    started: AtomicUsize,
    fail_at: Option<i64>,
}

impl Fake {
    fn new(open: bool) -> Arc<Fake> {
        Self::failing(open, None)
    }

    fn failing(open: bool, fail_at: Option<i64>) -> Arc<Fake> {
        Arc::new(Fake {
            calls: Mutex::new(Vec::new()),
            gate: (Mutex::new(open), Condvar::new()),
            started: AtomicUsize::new(0),
            fail_at,
        })
    }

    fn open(&self) {
        *self.gate.0.lock().unwrap() = true;
        self.gate.1.notify_all();
    }

    fn calls(&self) -> Vec<(Kind, i64)> {
        self.calls.lock().unwrap().clone()
    }
}

impl Decoder for Arc<Fake> {
    fn load(&self, kind: Kind, time: CaptureTime) -> Result<RgbaImage, String> {
        self.started.fetch_add(1, Ordering::SeqCst);
        let mut open = self.gate.0.lock().unwrap();
        while !*open {
            open = self.gate.1.wait(open).unwrap();
        }
        drop(open);
        self.calls.lock().unwrap().push((kind, time.0));
        if self.fail_at == Some(time.0) {
            return Err("boom".into());
        }
        Ok(RgbaImage::new(2, 2, vec![1; 16]).unwrap())
    }
}

fn wait_until(what: &str, mut cond: impl FnMut() -> bool) {
    let end = Instant::now() + Duration::from_secs(10);
    while !cond() {
        assert!(Instant::now() < end, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn loader(fake: &Arc<Fake>, workers: usize) -> (ImageLoader, Arc<AtomicUsize>) {
    let repaints = Arc::new(AtomicUsize::new(0));
    let r = Arc::clone(&repaints);
    let l = ImageLoader::with_limits(
        Arc::new(Arc::clone(fake)),
        move || {
            r.fetch_add(1, Ordering::SeqCst);
        },
        workers,
        1 << 20,
        1 << 20,
    );
    (l, repaints)
}

fn collect(l: &ImageLoader, n: usize) -> Vec<Loaded> {
    let mut out = Vec::new();
    wait_until("results", || {
        out.extend(l.poll());
        out.len() >= n
    });
    out
}

#[test]
fn the_current_position_is_served_before_its_neighbours() {
    let fake = Fake::new(false);
    let (l, repaints) = loader(&fake, 1);
    // Occupy the only worker so that the rest queues up.
    assert!(matches!(
        l.request(Kind::Full, CaptureTime(10_000)),
        Status::Pending
    ));
    wait_until("worker busy", || fake.started.load(Ordering::SeqCst) == 1);
    l.set_position(CaptureTime(500));
    for t in [900, 100, 300, 500, 700, 200, 800] {
        l.request(Kind::Full, CaptureTime(t));
    }
    // The thumbnail of the current position goes before its full image.
    l.request(Kind::Thumb, CaptureTime(500));
    fake.open();
    collect(&l, 9);
    let calls = fake.calls();
    assert_eq!(calls[0], (Kind::Full, 10_000));
    assert_eq!(calls[1], (Kind::Thumb, 500));
    assert_eq!(calls[2], (Kind::Full, 500));
    let rest: Vec<i64> = calls[3..].iter().map(|c| c.1).collect();
    // Then by distance from 500: 300/700 (200), 200/800 (300), 100/900 (400).
    for pair in rest.chunks(2) {
        assert_eq!(pair.iter().sum::<i64>(), 1000);
    }
    assert!(rest[..2].contains(&300) && rest[2..4].contains(&200) && rest[4..].contains(&100));
    assert!(repaints.load(Ordering::SeqCst) >= 9);
}

#[test]
fn after_a_jump_stale_jobs_are_dropped_and_not_delivered() {
    let fake = Fake::new(false);
    let (l, _) = loader(&fake, 1);
    l.request(Kind::Full, CaptureTime(1)); // running, will be stale
    wait_until("worker busy", || fake.started.load(Ordering::SeqCst) == 1);
    l.request(Kind::Full, CaptureTime(2)); // queued, will be stale
    l.request(Kind::Thumb, CaptureTime(3)); // queued, will be stale
    l.jump(CaptureTime(1000));
    assert_eq!(l.pending(), 0);
    l.request(Kind::Full, CaptureTime(1000));
    fake.open();
    let got = collect(&l, 1);
    // Let any (wrongly) pending extra result show up.
    std::thread::sleep(Duration::from_millis(100));
    let mut all = got;
    all.extend(l.poll());
    assert_eq!(all.len(), 1, "only the post-jump job is delivered");
    assert_eq!(all[0].time, CaptureTime(1000));
    // The queued stale jobs never reached the decoder; the running one
    // finished but its result was thrown away (not cached either).
    let decoded: Vec<i64> = fake.calls().iter().map(|c| c.1).collect();
    assert!(!decoded.contains(&2) && !decoded.contains(&3));
    assert_eq!(l.cached_len(), (1, 0));
    assert!(matches!(
        l.request(Kind::Full, CaptureTime(1)),
        Status::Pending
    ));
}

#[test]
fn request_returns_fast_while_workers_are_busy() {
    let fake = Fake::new(false);
    let (l, _) = loader(&fake, 2);
    for t in 0..2 {
        l.request(Kind::Full, CaptureTime(t));
    }
    wait_until("workers busy", || fake.started.load(Ordering::SeqCst) == 2);
    let mut worst = Duration::ZERO;
    for t in 10..400 {
        let start = Instant::now();
        let s = l.request(Kind::Thumb, CaptureTime(t));
        worst = worst.max(start.elapsed());
        assert!(matches!(s, Status::Pending));
    }
    assert!(
        worst < Duration::from_millis(1),
        "slowest request: {worst:?}"
    );
    fake.open();
}

#[test]
fn lock_clears_both_caches_and_the_pending_jobs() {
    let fake = Fake::new(true);
    let (l, _) = loader(&fake, 2);
    l.request(Kind::Full, CaptureTime(1));
    l.request(Kind::Thumb, CaptureTime(1));
    collect(&l, 2);
    assert_eq!(l.cached_len(), (1, 1));
    assert!(l.cached_bytes().0 > 0 && l.cached_bytes().1 > 0);
    assert!(matches!(
        l.request(Kind::Full, CaptureTime(1)),
        Status::Done(Item::Ready(_))
    ));

    // Pending jobs: close the gate and queue more.
    let fake2 = Fake::new(false);
    let (l2, _) = loader(&fake2, 1);
    l2.request(Kind::Full, CaptureTime(1));
    wait_until("busy", || fake2.started.load(Ordering::SeqCst) == 1);
    for t in 2..10 {
        l2.request(Kind::Thumb, CaptureTime(t));
    }
    assert_eq!(l2.pending(), 9);

    // Both go through the hook that the Session runs on lock.
    let hooks: Vec<Box<dyn FnOnce() + Send>> =
        vec![Box::new(l.lock_hook()), Box::new(l2.lock_hook())];
    for h in hooks {
        h();
    }
    assert_eq!(l.cached_len(), (0, 0));
    assert_eq!(l.cached_bytes(), (0, 0));
    assert_eq!(l2.pending(), 0);
    assert!(matches!(
        l.request(Kind::Full, CaptureTime(1)),
        Status::Closed
    ));
    fake2.open();
    std::thread::sleep(Duration::from_millis(50));
    assert!(l2.poll().is_empty(), "a result after lock was delivered");
    assert_eq!(l2.cached_len(), (0, 0), "a result after lock was cached");
    // Only the one running job ever decoded.
    assert_eq!(fake2.calls().len(), 1);
}

#[test]
fn errors_become_items_and_the_loader_keeps_working() {
    let fake = Fake::failing(true, Some(2));
    let (l, _) = loader(&fake, 1);
    l.request(Kind::Full, CaptureTime(2));
    l.request(Kind::Full, CaptureTime(3));
    let got = collect(&l, 2);
    assert!(got
        .iter()
        .any(|r| r.time == CaptureTime(2) && matches!(&r.item, Item::Failed(m) if m == "boom")));
    assert!(got
        .iter()
        .any(|r| r.time == CaptureTime(3) && matches!(r.item, Item::Ready(_))));
    // The error is remembered and not retried in a loop.
    assert!(matches!(
        l.request(Kind::Full, CaptureTime(2)),
        Status::Done(Item::Failed(_))
    ));
    assert_eq!(fake.calls().len(), 2);
}

#[test]
fn prefetch_requests_three_full_images_and_forty_thumbnails_per_side() {
    let fake = Fake::new(false);
    let (l, _) = loader(&fake, 1);
    let index = Index::from_times((0..200).map(|i| CaptureTime(i * 1000)).collect());
    l.prefetch(&index, CaptureTime(100_000));
    // current + 3 on each side (7 full), current + 40 on each side (81 thumbs).
    assert_eq!(l.pending(), 7 + 81);
    // At the start of the index only the existing neighbours are requested.
    let (l2, _) = loader(&fake, 1);
    l2.prefetch(&index, CaptureTime(0));
    assert_eq!(l2.pending(), 4 + 41);
    fake.open();
}
