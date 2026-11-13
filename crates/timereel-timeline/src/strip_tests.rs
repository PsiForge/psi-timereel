//! Tests of the strip geometry, the request plan, the labels and the
//! metadata cache.

#![allow(clippy::unwrap_used)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use timereel_store::CaptureMeta;

use crate::*;

const MIN: i64 = 60_000;
const INTERVAL: Duration = Duration::from_secs(300);

fn times(n: usize) -> Vec<CaptureTime> {
    (0..n as i64).map(|i| CaptureTime(i * 5 * MIN)).collect()
}

fn meta(time: CaptureTime) -> CaptureMeta {
    CaptureMeta {
        taken_at_ms: time.0,
        tz_offset_min: 120,
        app_name: "Editor".into(),
        window_title: "notes.txt".into(),
        width: 4,
        height: 4,
        format: "webp".into(),
    }
}

#[test]
fn the_layout_adds_a_marker_slot_after_each_break() {
    // 0, 5, 10 min, a break, 40, 45 min.
    let t: Vec<_> = [0, 5, 10, 40, 45]
        .iter()
        .map(|m| CaptureTime(m * MIN))
        .collect();
    let layout = StripLayout::new(&t, INTERVAL, 100.0, 60.0, 10.0);
    assert_eq!(layout.len(), 5);
    assert_eq!(layout.left(1), 110.0);
    assert_eq!(layout.left(2), 220.0);
    // The break follows item 2: item 3 starts after the marker and spacing.
    assert_eq!(layout.gap_left(2), 330.0);
    assert_eq!(layout.left(3), 330.0 + 60.0 + 10.0);
    assert_eq!(layout.gaps_near(0..5).len(), 1);
    assert_eq!(layout.gaps_near(0..5)[0].0, 2);
    assert_eq!(layout.gaps_near(0..5)[0].1.length_ms(), 30 * MIN);
    assert!(layout.total_width() > layout.left(4) + 100.0);
    // Exactly 1.5 x the interval is not a break.
    let t: Vec<_> = [0, 7].iter().map(|m| CaptureTime(m * MIN)).collect();
    let layout = StripLayout::new(&t, Duration::from_secs(300), 10.0, 5.0, 1.0);
    assert_eq!(layout.gaps_near(0..2).len(), 0);
}

#[test]
fn only_the_thumbnails_in_the_window_are_visible() {
    let layout = StripLayout::new(&times(8600), INTERVAL, 100.0, 60.0, 10.0);
    let v = layout.visible(1100.0, 500.0);
    // Item i spans [110 i, 110 i + 100]: 10 (1100..1200) to 14 (1540..1640).
    assert_eq!(v, 10..15);
    assert!(layout.visible(0.0, 50.0) == (0..1));
    assert_eq!(layout.visible(1e9, 500.0).len(), 0);
    assert!(layout.visible(105.0, 0.0).is_empty());
    // A partly visible thumbnail counts.
    assert_eq!(layout.visible(150.0, 10.0), 1..2);
    assert_eq!(layout.hit(115.0), Some(1));
    assert_eq!(layout.hit(215.0), None, "space between thumbnails");
    assert_eq!(layout.hit(-5.0), None);
    assert_eq!(layout.hit(1e9), None);
}

#[test]
fn gaps_near_the_visible_range_include_the_marker_before_it() {
    let t: Vec<_> = [0, 5, 40, 45, 50, 90]
        .iter()
        .map(|m| CaptureTime(m * MIN))
        .collect();
    let layout = StripLayout::new(&t, INTERVAL, 100.0, 60.0, 10.0);
    // Visible 2..4: the marker after item 1 sits just left of item 2.
    let near: Vec<usize> = layout.gaps_near(2..4).iter().map(|(i, _)| *i).collect();
    assert_eq!(near, vec![1]);
    let near: Vec<usize> = layout.gaps_near(0..6).iter().map(|(i, _)| *i).collect();
    assert_eq!(near, vec![1, 4]);
}

#[test]
fn requests_cover_the_visible_range_plus_the_prefetch_only() {
    let plan = plan_requests(100..110, 105, 1000);
    assert_eq!(
        plan.thumbs,
        (100 - PREFETCH_THUMBS)..(110 + PREFETCH_THUMBS)
    );
    assert_eq!(plan.full, 102..109);
    // Clamped at both ends.
    let plan = plan_requests(0..5, 0, 8);
    assert_eq!(plan.thumbs, 0..8);
    assert_eq!(plan.full, 0..4);
    let plan = plan_requests(995..1000, 999, 1000);
    assert_eq!(plan.thumbs, (995 - PREFETCH_THUMBS)..1000);
    assert_eq!(plan.full, 996..1000);
    // The selected thumbnail is requested even if the strip shows nothing.
    let plan = plan_requests(0..0, 500, 1000);
    assert_eq!(
        plan.thumbs,
        (500 - PREFETCH_THUMBS)..(501 + PREFETCH_THUMBS)
    );
    assert_eq!(plan_requests(0..0, 0, 0).thumbs, 0..0);
    assert_eq!(plan_requests(0..0, 0, 0).full, 0..0);
}

#[test]
fn labels_use_the_recorded_utc_offset() {
    // 2025-05-14 09:30:15 UTC.
    let t = CaptureTime(1_747_215_015_000);
    assert_eq!(format_clock(t, 0), "09:30:15");
    assert_eq!(format_clock(t, 120), "11:30:15");
    assert_eq!(format_clock(t, -90), "08:00:15");
    assert_eq!(format_date_time(t, 120), "2025-05-14 11:30:15");
    assert_eq!(format_date_time(t, 15 * 60), "2025-05-15 00:30:15");
    assert_eq!(format_clock(t, i32::MAX), "--:--:--");
}

#[test]
fn gap_lengths_are_written_in_minutes_hours_and_days() {
    assert_eq!(format_gap(8 * MIN), "8 min");
    assert_eq!(format_gap(25 * MIN + 20_000), "25 min");
    assert_eq!(format_gap(60 * MIN), "1 h");
    assert_eq!(format_gap(125 * MIN), "2 h 5 min");
    assert_eq!(format_gap(27 * 60 * MIN), "1 d 3 h");
    assert_eq!(format_gap(48 * 60 * MIN), "2 d");
    assert_eq!(format_gap(-5), "0 min");
}

struct FakeMeta {
    loads: AtomicUsize,
    fail: Option<CaptureTime>,
    gate: Mutex<()>,
}

impl MetaSource for FakeMeta {
    fn load(&self, time: CaptureTime) -> Result<CaptureMeta, String> {
        drop(self.gate.lock().unwrap());
        self.loads.fetch_add(1, Ordering::SeqCst);
        if self.fail == Some(time) {
            return Err("unreadable".into());
        }
        Ok(meta(time))
    }
}

fn fake(fail: Option<CaptureTime>) -> Arc<FakeMeta> {
    Arc::new(FakeMeta {
        loads: AtomicUsize::new(0),
        fail,
        gate: Mutex::new(()),
    })
}

fn wait_for(what: &str, done: impl Fn() -> bool) {
    let start = Instant::now();
    while !done() {
        assert!(
            start.elapsed() < Duration::from_secs(20),
            "timed out: {what}"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn metadata_is_loaded_in_the_background_and_repaints() {
    let source = fake(Some(CaptureTime(5 * MIN)));
    let repaints = Arc::new(AtomicUsize::new(0));
    let r = Arc::clone(&repaints);
    let cache = MetaCache::new(source.clone(), move || {
        r.fetch_add(1, Ordering::SeqCst);
    });
    let t = times(150);
    cache.queue(t.iter().rev().copied().collect());
    wait_for("all loaded", || cache.len() == 150);
    assert_eq!(cache.get(t[0]), Some(MetaEntry::Ready(meta(t[0]))));
    assert_eq!(
        cache.get(t[1]),
        Some(MetaEntry::Failed("unreadable".into()))
    );
    wait_for("final repaint", || repaints.load(Ordering::SeqCst) >= 3);

    // Queuing known times again reads nothing.
    let before = source.loads.load(Ordering::SeqCst);
    cache.queue(t.clone());
    cache.queue(vec![CaptureTime(1_000_000 * MIN)]);
    wait_for("new one", || {
        cache.get(CaptureTime(1_000_000 * MIN)).is_some()
    });
    assert_eq!(source.loads.load(Ordering::SeqCst), before + 1);
}

#[test]
fn the_lock_hook_empties_the_cache_and_stops_the_worker() {
    let source = fake(None);
    let cache = MetaCache::new(source.clone(), || {});
    cache.queue(times(10));
    wait_for("loaded", || cache.len() == 10);
    let hook = cache.lock_hook();
    hook();
    assert!(cache.is_empty());
    assert_eq!(cache.get(CaptureTime(0)), None);
    // Later batches are ignored.
    let before = source.loads.load(Ordering::SeqCst);
    cache.queue(times(20));
    std::thread::sleep(Duration::from_millis(50));
    assert!(cache.is_empty());
    assert_eq!(source.loads.load(Ordering::SeqCst), before);
}

#[test]
fn locking_during_a_batch_leaves_nothing_behind() {
    let source = fake(None);
    let gate = source.gate.lock().unwrap();
    let cache = MetaCache::new(source.clone(), || {});
    cache.queue(times(50));
    std::thread::sleep(Duration::from_millis(30));
    let hook = cache.lock_hook();
    hook();
    drop(gate);
    drop(cache); // joins the worker
    assert!(source.loads.load(Ordering::SeqCst) <= 1);
}
