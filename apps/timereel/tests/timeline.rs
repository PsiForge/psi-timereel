#![allow(clippy::unwrap_used)]
//! egui_kittest tests of the timeline: the real viewer with encrypted
//! captures (keys, labels, gap markers) and the view alone with a recording
//! decoder (what gets requested).

mod common;

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use common::*;
use egui_kittest::kittest::Queryable;
use egui_kittest::Harness;
use timereel::autolock::FakeClock;
use timereel::session::Session;
use timereel::timeline_ui::*;
use timereel_image::{encode, Format, RgbaImage};
use timereel_store::{CaptureMeta, CaptureTime, Store};
use timereel_timeline::{
    plan_requests, Decoder, Index, Kind, MetaSource, PREFETCH_FULL, PREFETCH_THUMBS,
};

const PASSWORD: &str = "correct horse battery";
const MIN: i64 = 60_000;
/// 2025-05-14 09:00:00 UTC.
const T0: i64 = 1_747_213_200_000;
const INTERVAL: Duration = Duration::from_secs(300);

fn tiny_image(shade: u8) -> RgbaImage {
    RgbaImage::new(16, 9, [shade, 40, 200, 255].repeat(16 * 9)).expect("image")
}

/// Writes one capture; the local time zone is UTC+2.
fn write_capture(store: &Store, public_key: &str, minutes: i64, app: &str, title: &str) {
    let image = encode(&tiny_image(minutes as u8), Format::Webp, 50).expect("encode");
    let meta = CaptureMeta {
        taken_at_ms: T0 + minutes * MIN,
        tz_offset_min: 120,
        app_name: app.into(),
        window_title: title.into(),
        width: 16,
        height: 9,
        format: "webp".into(),
    };
    store
        .write(public_key, &meta, &image, &image)
        .expect("write capture");
}

/// A viewer over real encrypted captures at `minutes` after `T0`, unlocked.
fn unlocked_viewer(dir: &tempfile::TempDir, minutes: &[i64]) -> (ViewerScreen, Vec<CaptureTime>) {
    let (path, _) = make_keyring(dir.path(), PASSWORD);
    let public_key = timereel_crypto::load(&path)
        .expect("keyring")
        .public_key()
        .to_owned();
    let store = Store::new(dir.path());
    for &m in minutes {
        write_capture(
            &store,
            &public_key,
            m,
            &format!("App{m}"),
            &format!("Title of {m}"),
        );
    }
    let mut h = viewer_harness(path, FakeClock::default());
    unlock(&mut h, PASSWORD, false);
    let times: Vec<_> = minutes.iter().map(|m| CaptureTime(T0 + m * MIN)).collect();
    let last = *times.last().expect("captures");
    wait_until(&mut h, "newest capture's details", |h| {
        has_label(h, &format!("App{}", (last.0 - T0) / MIN))
    });
    (h, times)
}

fn selected(h: &ViewerScreen) -> CaptureTime {
    h.state()
        .timeline()
        .expect("timeline")
        .selected()
        .expect("selection")
}

#[test]
fn arrow_keys_move_the_selection_to_the_previous_and_next_capture() {
    let dir = tempfile::tempdir().expect("temp dir");
    let (mut h, times) = unlocked_viewer(&dir, &[0, 5, 10, 15]);
    assert_eq!(selected(&h), times[3], "the newest capture starts selected");

    h.key_press(egui::Key::ArrowLeft);
    h.run_steps(2);
    assert_eq!(selected(&h), times[2]);
    h.key_press(egui::Key::ArrowLeft);
    h.run_steps(2);
    assert_eq!(selected(&h), times[1]);
    h.key_press(egui::Key::ArrowRight);
    h.run_steps(2);
    assert_eq!(selected(&h), times[2]);

    // The ends stop the selection.
    for _ in 0..6 {
        h.key_press(egui::Key::ArrowRight);
        h.run_steps(1);
    }
    assert_eq!(selected(&h), times[3]);
    for _ in 0..6 {
        h.key_press(egui::Key::ArrowLeft);
        h.run_steps(1);
    }
    assert_eq!(selected(&h), times[0]);
}

#[test]
fn the_large_view_shows_time_app_and_title_of_the_selected_capture() {
    let dir = tempfile::tempdir().expect("temp dir");
    let (mut h, _) = unlocked_viewer(&dir, &[0, 5, 10]);
    // 09:10 UTC is 11:10 in the recorded UTC+2.
    assert!(has_label(&h, "2025-05-14 11:10:00"));
    assert!(has_label(&h, "App10"));
    assert!(has_label(&h, "Title of 10"));
    assert!(!has_label(&h, "App5"));

    h.key_press(egui::Key::ArrowLeft);
    wait_until(&mut h, "previous capture's labels", |h| {
        has_label(h, "App5") && has_label(h, "Title of 5") && has_label(h, "2025-05-14 11:05:00")
    });
    assert!(!has_label(&h, "App10"));
    assert!(!has_label(&h, "Title of 10"));
}

#[test]
fn a_gap_marker_shows_only_between_captures_more_than_one_and_a_half_intervals_apart() {
    let dir = tempfile::tempdir().expect("temp dir");
    // 5, 5, then 7 (not a break), then 30 minutes (a break).
    let (h, _) = unlocked_viewer(&dir, &[0, 5, 10, 17, 47]);
    let markers = h.query_all_by_label_contains(GAP_LABEL_PREFIX).count();
    assert_eq!(markers, 1, "one break in the strip");
    assert!(has_label(&h, &gap_label(30 * MIN)));
    assert!(has_label(&h, "Gap: 30 min"));
}

#[test]
fn clicking_a_thumbnail_selects_it() {
    let dir = tempfile::tempdir().expect("temp dir");
    let (mut h, times) = unlocked_viewer(&dir, &[0, 5, 10]);
    // The strip labels use the recorded UTC+2 offset, once it is known.
    wait_until(&mut h, "strip labels", |h| has_label(h, "Capture 11:00:00"));
    h.get_by_label("Capture 11:00:00").click();
    h.run_steps(2);
    assert_eq!(selected(&h), times[0]);
    wait_until(&mut h, "its labels", |h| has_label(h, "App0"));
}

// ---- the view alone, with fake sources ------------------------------------

#[derive(Default)]
struct Recording {
    loads: Mutex<Vec<(Kind, CaptureTime)>>,
    hold_full: AtomicBool,
}

impl Recording {
    fn set(&self, kind: Kind) -> BTreeSet<i64> {
        self.loads
            .lock()
            .unwrap()
            .iter()
            .filter(|(k, _)| *k == kind)
            .map(|(_, t)| t.0)
            .collect()
    }
}

impl Decoder for Recording {
    fn load(&self, kind: Kind, time: CaptureTime) -> Result<RgbaImage, String> {
        if kind == Kind::Full {
            let start = Instant::now();
            while self.hold_full.load(Ordering::SeqCst) && start.elapsed() < Duration::from_secs(20)
            {
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        self.loads.lock().unwrap().push((kind, time));
        Ok(tiny_image((time.0 / MIN) as u8))
    }
}

struct FakeMeta;

impl MetaSource for FakeMeta {
    fn load(&self, time: CaptureTime) -> Result<CaptureMeta, String> {
        Ok(CaptureMeta {
            taken_at_ms: time.0,
            tz_offset_min: 0,
            app_name: "Editor".into(),
            window_title: "notes".into(),
            width: 16,
            height: 9,
            format: "webp".into(),
        })
    }
}

fn times(n: i64) -> Vec<CaptureTime> {
    (0..n).map(|i| CaptureTime(T0 + i * 5 * MIN)).collect()
}

fn view_harness(
    decoder: Arc<Recording>,
    n: i64,
) -> (Harness<'static, Option<TimelineView>>, Vec<CaptureTime>) {
    let t = times(n);
    let ctx = egui::Context::default();
    let view = TimelineView::new(
        Index::from_times(t.clone()),
        decoder,
        Arc::new(FakeMeta),
        INTERVAL,
        &ctx,
    );
    let mut h = Harness::builder()
        .with_size(egui::vec2(800.0, 600.0))
        .build_ui_state(
            |ui, view: &mut Option<TimelineView>| {
                if let Some(view) = view {
                    view.ui(ui);
                }
            },
            Some(view),
        );
    h.run_steps(2);
    (h, t)
}

/// Steps until the number of loads has not changed for a while.
fn settle(h: &mut Harness<'static, Option<TimelineView>>, decoder: &Recording) {
    let mut last = usize::MAX;
    let mut stable = 0;
    let start = Instant::now();
    while stable < 10 {
        assert!(
            start.elapsed() < Duration::from_secs(30),
            "loading did not settle"
        );
        std::thread::sleep(Duration::from_millis(20));
        h.run_steps(1);
        let now = decoder.loads.lock().unwrap().len();
        stable = if now == last { stable + 1 } else { 0 };
        last = now;
    }
}

fn view<'a>(h: &'a Harness<'static, Option<TimelineView>>) -> &'a TimelineView {
    h.state().as_ref().expect("view")
}

fn thumb_indexes(rec: &Recording, t: &[CaptureTime]) -> BTreeSet<usize> {
    rec.set(Kind::Thumb)
        .iter()
        .map(|ms| t.iter().position(|c| c.0 == *ms).expect("known capture"))
        .collect()
}

fn full_indexes(rec: &Recording, t: &[CaptureTime]) -> BTreeSet<usize> {
    rec.set(Kind::Full)
        .iter()
        .map(|ms| t.iter().position(|c| c.0 == *ms).expect("known capture"))
        .collect()
}

#[test]
fn the_strip_requests_thumbnails_only_for_the_visible_range_plus_the_prefetch() {
    let rec = Arc::new(Recording::default());
    let (mut h, t) = view_harness(rec.clone(), 600);
    // Go to the middle of a day of captures.
    h.state_mut().as_mut().unwrap().select(t[300]);
    settle(&mut h, &rec);

    let visible = view(&h).visible_range();
    assert!(visible.contains(&300));
    assert!(visible.len() < 20, "only a few thumbnails fit: {visible:?}");
    let plan = plan_requests(visible.clone(), 300, t.len());
    assert_eq!(
        plan.thumbs,
        (visible.start - PREFETCH_THUMBS)..(visible.end + PREFETCH_THUMBS)
    );
    let asked = thumb_indexes(&rec, &t);
    let allowed: BTreeSet<usize> = plan.thumbs.clone().collect();
    // Thumbnails of the first selection (the newest capture) were also
    // wanted at first; the loader drops them as stale after the jump.
    let recent: BTreeSet<usize> = asked.intersection(&allowed).copied().collect();
    assert_eq!(
        recent, allowed,
        "every visible and prefetch thumbnail arrives"
    );
    assert!(
        asked
            .iter()
            .all(|i| allowed.contains(i) || *i >= 600 - PREFETCH_THUMBS - 20),
        "no thumbnail outside the plan (except the first view of the newest): {asked:?}"
    );
    assert!(asked.len() < 160);
}

#[test]
fn a_full_image_is_requested_only_for_the_selected_capture_and_its_prefetch_neighbours() {
    let rec = Arc::new(Recording::default());
    let (mut h, t) = view_harness(rec.clone(), 600);
    h.state_mut().as_mut().unwrap().select(t[300]);
    settle(&mut h, &rec);
    let around =
        |c: usize| -> BTreeSet<usize> { (c - PREFETCH_FULL..=c + PREFETCH_FULL).collect() };
    let asked = full_indexes(&rec, &t);
    assert!(around(300).is_subset(&asked));
    let near = around(300);
    let extra: Vec<_> = asked.difference(&near).collect();
    // Only the first view of the newest capture adds anything (the jump
    // dropped those as stale).
    assert!(
        extra.iter().all(|i| **i >= 600 - 1 - PREFETCH_FULL),
        "{extra:?}"
    );

    // One step right adds exactly one new full image.
    h.key_press(egui::Key::ArrowRight);
    h.run_steps(2);
    settle(&mut h, &rec);
    let after = full_indexes(&rec, &t);
    let new: Vec<_> = after.difference(&asked).collect();
    assert_eq!(new, vec![&(300 + 1 + PREFETCH_FULL)]);
    assert_eq!(view(&h).selected(), Some(t[301]));
}

#[test]
fn the_thumbnail_stands_in_while_the_full_image_loads() {
    let rec = Arc::new(Recording::default());
    rec.hold_full.store(true, Ordering::SeqCst);
    let (mut h, _) = view_harness(rec.clone(), 5);
    wait_until(&mut h, "the thumbnail stand-in", |h| {
        has_label(h, THUMB_IMAGE_TEXT)
    });
    assert!(!has_label(&h, FULL_IMAGE_TEXT));
    rec.hold_full.store(false, Ordering::SeqCst);
    wait_until(&mut h, "the full image", |h| has_label(h, FULL_IMAGE_TEXT));
    assert!(!has_label(&h, THUMB_IMAGE_TEXT));
}

#[test]
fn the_mouse_wheel_moves_the_selection() {
    let rec = Arc::new(Recording::default());
    let (mut h, t) = view_harness(rec, 50);
    h.state_mut().as_mut().unwrap().select(t[10]);
    h.run_steps(2);
    let wheel = |h: &Harness<'static, Option<TimelineView>>, y: f32| {
        h.event(egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            source: egui::MouseWheelSource::Wheel,
            delta: egui::vec2(0.0, y),
            phase: egui::TouchPhase::Move,
            modifiers: egui::Modifiers::NONE,
        });
    };
    wheel(&h, -40.0);
    h.run_steps(10);
    assert_eq!(view(&h).selected(), Some(t[11]), "wheel down goes forward");
    wheel(&h, -80.0);
    h.run_steps(10);
    assert_eq!(view(&h).selected(), Some(t[13]));
    wheel(&h, 40.0);
    h.run_steps(10);
    assert_eq!(view(&h).selected(), Some(t[12]), "wheel up goes back");
}

#[test]
fn an_empty_store_shows_a_message() {
    let rec = Arc::new(Recording::default());
    let (h, _) = view_harness(rec, 0);
    assert!(has_label(&h, EMPTY_TIMELINE_MESSAGE));
}

#[test]
fn locking_the_session_clears_the_metadata_and_frees_the_textures() {
    let rec = Arc::new(Recording::default());
    let (mut h, t) = view_harness(rec.clone(), 20);
    settle(&mut h, &rec);
    wait_until(&mut h, "metadata", |h| view(h).meta().len() == t.len());

    let mut session = Session::new(());
    view(&h).register_lock_hooks(&mut session);
    session.lock();
    assert!(view(&h).meta().is_empty(), "metadata cleared on lock");

    // Dropping the view (what the viewer does when it locks) frees the
    // pictures on the GPU.
    let before = h.ctx.tex_manager().read().num_allocated();
    *h.state_mut() = None;
    h.run_steps(2);
    let after = h.ctx.tex_manager().read().num_allocated();
    assert!(after < before, "textures freed: {before} -> {after}");
}
