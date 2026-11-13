#![allow(clippy::unwrap_used)]
//! egui_kittest tests of the timeline search: typing in the box, moving
//! within the matches, clearing, and the loading progress.

mod common;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use common::{has_label, wait_until};
use egui::accesskit::Role;
use egui_kittest::kittest::Queryable;
use egui_kittest::Harness;
use timereel::timeline_ui::*;
use timereel_image::RgbaImage;
use timereel_store::{CaptureMeta, CaptureTime};
use timereel_timeline::{Decoder, Index, Kind, MetaSource};

const MIN: i64 = 60_000;
/// 2025-05-14 09:00:00 UTC.
const T0: i64 = 1_747_213_200_000;
const INTERVAL: Duration = Duration::from_secs(300);

struct Pixels;

impl Decoder for Pixels {
    fn load(&self, _: Kind, _: CaptureTime) -> Result<RgbaImage, String> {
        Ok(RgbaImage::new(16, 9, [10, 40, 200, 255].repeat(16 * 9)).expect("image"))
    }
}

/// Every third capture (0, 3, 6, ...) is `Editor`, the others `Browser`; the
/// title says which number it is. Loading waits while `hold` is set.
#[derive(Default)]
struct Meta {
    hold: AtomicBool,
}

impl MetaSource for Meta {
    fn load(&self, time: CaptureTime) -> Result<CaptureMeta, String> {
        while self.hold.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(5));
        }
        let n = (time.0 - T0) / (5 * MIN);
        Ok(CaptureMeta {
            taken_at_ms: time.0,
            tz_offset_min: 0,
            app_name: if n % 3 == 0 { "Editor" } else { "Browser" }.into(),
            window_title: format!("Número {n}"),
            width: 16,
            height: 9,
            format: "webp".into(),
        })
    }
}

type H = Harness<'static, TimelineView>;

fn harness(n: i64, meta: Arc<Meta>) -> (H, Vec<CaptureTime>) {
    let times: Vec<_> = (0..n).map(|i| CaptureTime(T0 + i * 5 * MIN)).collect();
    let ctx = egui::Context::default();
    let view = TimelineView::new(
        Index::from_times(times.clone()),
        Arc::new(Pixels),
        meta,
        INTERVAL,
        &ctx,
    );
    let mut h = Harness::builder()
        .with_size(egui::vec2(900.0, 600.0))
        .build_ui_state(|ui, view: &mut TimelineView| view.ui(ui), view);
    h.run_steps(2);
    (h, times)
}

fn loaded(h: &mut H, n: usize) {
    wait_until(h, "all details", |h| h.state().meta().len() == n);
}

fn selected(h: &H) -> CaptureTime {
    h.state().selected().expect("selection")
}

/// Types into the search box like a user.
fn type_query(h: &mut H, text: &str) {
    h.get_by_role(Role::TextInput).focus();
    h.run_steps(2);
    h.get_by_role(Role::TextInput).type_text(text);
    h.run_steps(3);
}

#[test]
fn clearing_the_filter_keeps_the_same_capture_selected_in_the_full_timeline() {
    let (mut h, t) = harness(20, Arc::default());
    loaded(&mut h, 20);
    h.state_mut().select(t[6]); // an Editor capture
    h.run_steps(2);

    type_query(&mut h, "editor");
    assert!(h.state().is_filtering());
    let editors: Vec<_> = t.iter().copied().step_by(3).collect();
    assert_eq!(h.state().shown_times(), editors.as_slice());
    assert_eq!(selected(&h), t[6], "a matching capture stays selected");
    assert!(has_label(&h, &format!("{} matches", editors.len())));

    // Navigation moves within the matches (Enter leaves the search box, so
    // that the arrow keys are for the timeline again).
    h.key_press(egui::Key::Enter);
    h.run_steps(3);
    h.key_press(egui::Key::ArrowRight);
    h.run_steps(2);
    assert_eq!(selected(&h), t[9]);
    h.get_by_label(CLEAR_SEARCH_LABEL).click();
    h.run_steps(3);

    assert!(!h.state().is_filtering());
    assert_eq!(h.state().query(), "");
    assert_eq!(h.state().shown_times(), t.as_slice(), "the full timeline");
    assert_eq!(selected(&h), t[9], "the same capture is still selected");
    // The arrow keys move one capture again.
    h.key_press(egui::Key::ArrowRight);
    h.run_steps(2);
    assert_eq!(selected(&h), t[10]);
}

#[test]
fn clearing_a_filter_that_matched_nothing_changes_no_selection() {
    let (mut h, t) = harness(10, Arc::default());
    loaded(&mut h, 10);
    h.state_mut().select(t[4]);
    h.run_steps(2);
    type_query(&mut h, "zzz-no-such-app");
    assert!(has_label(&h, NO_MATCHES_MESSAGE));
    assert!(h.state().shown_times().is_empty());
    assert_eq!(selected(&h), t[4]);
    h.key_press(egui::Key::ArrowRight);
    h.run_steps(2);
    assert_eq!(selected(&h), t[4], "nothing to move to");
    h.state_mut().set_query("");
    h.run_steps(3);
    assert_eq!(h.state().shown_times(), t.as_slice());
    assert_eq!(selected(&h), t[4]);
}

#[test]
fn arrow_keys_and_the_wheel_skip_captures_that_do_not_match() {
    let (mut h, t) = harness(20, Arc::default());
    loaded(&mut h, 20);
    h.state_mut().select(t[0]);
    h.run_steps(2);
    // Matches the titles "Número 1" and "Número 10" to "Número 19", ignoring
    // case, with a non-ASCII letter.
    type_query(&mut h, "NÚMERO 1");
    let mut expected = vec![t[1]];
    expected.extend_from_slice(&t[10..]);
    assert_eq!(h.state().shown_times(), expected.as_slice());
    assert_eq!(selected(&h), t[1], "moved to the nearest match");

    h.state_mut().set_query("browser");
    h.run_steps(3);
    h.state_mut().select(t[1]);
    h.run_steps(2);
    h.state_mut().step(1);
    assert_eq!(selected(&h), t[2], "skips 3, the Editor capture");
    h.state_mut().step(1);
    assert_eq!(selected(&h), t[4]);
    h.state_mut().step(-2);
    assert_eq!(selected(&h), t[1]);
    h.state_mut().step(-5);
    assert_eq!(selected(&h), t[1], "stops at the first match");
}

#[test]
fn the_arrow_keys_belong_to_the_search_box_while_it_has_the_focus() {
    let (mut h, t) = harness(10, Arc::default());
    loaded(&mut h, 10);
    h.state_mut().select(t[4]);
    h.run_steps(2);
    h.get_by_role(Role::TextInput).focus();
    h.run_steps(3);
    h.key_press(egui::Key::ArrowRight);
    h.run_steps(3);
    assert_eq!(selected(&h), t[4], "the key moved the text cursor");
}

#[test]
fn the_progress_shows_while_details_load_and_searching_uses_the_part_loaded() {
    let meta = Arc::new(Meta::default());
    meta.hold.store(true, Ordering::SeqCst);
    let (mut h, t) = harness(30, meta.clone());
    h.run_steps(3);
    assert!(
        h.query_by_label_contains(LOADING_PROGRESS_PREFIX).is_some(),
        "progress is shown"
    );
    // Nothing is loaded, so a query finds nothing yet.
    type_query(&mut h, "editor");
    assert!(h.state().shown_times().is_empty());
    assert_eq!(selected(&h), t[29]);

    meta.hold.store(false, Ordering::SeqCst);
    loaded(&mut h, 30);
    wait_until(&mut h, "the matches", |h| {
        h.state().shown_times().len() == 10
    });
    assert!(
        h.query_by_label_contains(LOADING_PROGRESS_PREFIX).is_none(),
        "progress is gone when loading is done"
    );
}
