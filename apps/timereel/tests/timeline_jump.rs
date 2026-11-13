#![allow(clippy::unwrap_used)]
//! egui_kittest tests of the jump controls (T21): the date picker fields and
//! Go, the quick-jump buttons, the 'showing nearest' message and the arrow
//! keys after a jump. The view runs alone over fake sources, in a fixed
//! UTC+2 time zone and with a fixed clock.

mod common;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{FixedOffset, NaiveDate};
use common::wait_until;
use egui_kittest::kittest::Queryable;
use egui_kittest::Harness;
use timereel::jump_ui::*;
use timereel::timeline_ui::TimelineView;
use timereel_image::RgbaImage;
use timereel_store::{CaptureMeta, CaptureTime};
use timereel_timeline::{Decoder, Index, Kind, MetaSource};

type H = Harness<'static, Option<TimelineView>>;

const MIN: i64 = 60_000;
const HOUR: i64 = 60 * MIN;
const DAY: i64 = 24 * HOUR;
/// 2025-05-12 00:00:00 UTC.
const D12: i64 = 1_747_008_000_000;
const OFFSET_SECONDS: i32 = 2 * 3600;

fn zone() -> FixedOffset {
    FixedOffset::east_opt(OFFSET_SECONDS).unwrap()
}

/// A capture at `hour`:`minute` local time (UTC+2) on May `day` 2025.
fn at(day: i64, hour: i64, minute: i64) -> CaptureTime {
    CaptureTime(D12 + (day - 12) * DAY + hour * HOUR + minute * MIN - 2 * HOUR)
}

fn date(day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(2025, 5, day).unwrap()
}

/// May 12: three captures. May 13: none. May 14: four captures. May 16: one.
fn captures() -> Vec<CaptureTime> {
    vec![
        at(12, 10, 0),
        at(12, 10, 30),
        at(12, 11, 0),
        at(14, 9, 0),
        at(14, 9, 30),
        at(14, 10, 30),
        at(14, 12, 0),
        at(16, 8, 15),
    ]
}

/// The clock of the tests: 2025-05-14 15:00 local.
fn now() -> CaptureTime {
    at(14, 15, 0)
}

#[derive(Default)]
struct Decoded(Mutex<Vec<(Kind, CaptureTime)>>);

impl Decoder for Decoded {
    fn load(&self, kind: Kind, time: CaptureTime) -> Result<RgbaImage, String> {
        self.0.lock().unwrap().push((kind, time));
        Ok(RgbaImage::new(16, 9, [90, 40, 200, 255].repeat(16 * 9)).expect("image"))
    }
}

struct Meta;

impl MetaSource for Meta {
    fn load(&self, time: CaptureTime) -> Result<CaptureMeta, String> {
        Ok(CaptureMeta {
            taken_at_ms: time.0,
            tz_offset_min: 120,
            app_name: "Editor".into(),
            window_title: "notes".into(),
            width: 16,
            height: 9,
            format: "webp".into(),
        })
    }
}

fn harness() -> (H, Arc<Decoded>) {
    let decoder = Arc::new(Decoded::default());
    let ctx = egui::Context::default();
    let mut view = TimelineView::new(
        Index::from_times(captures()),
        decoder.clone(),
        Arc::new(Meta),
        Duration::from_secs(300),
        &ctx,
    );
    let controls = view.jump_controls_mut();
    controls.set_zone(Zone::Fixed(zone()));
    controls.set_now(now());
    let mut h = Harness::builder()
        .with_size(egui::vec2(1000.0, 600.0))
        .build_ui_state(
            |ui, view: &mut Option<TimelineView>| {
                if let Some(view) = view {
                    view.ui(ui);
                }
            },
            Some(view),
        );
    h.run_steps(3);
    (h, decoder)
}

fn view(h: &H) -> &TimelineView {
    h.state().as_ref().expect("view")
}

fn selected(h: &H) -> CaptureTime {
    view(h).selected().expect("selection")
}

fn click(h: &mut H, label: &str) {
    h.get_by_label(label).click();
    h.run_steps(3);
}

/// What the T18 query says for `kind` when `current` is selected.
fn expected(kind: QuickJump, current: CaptureTime) -> CaptureTime {
    Index::from_times(captures())
        .quick_jump(kind, current, now(), &zone())
        .expect("landing")
        .time
}

#[test]
fn picking_a_date_and_time_selects_the_first_capture_at_or_after_it() {
    let (mut h, _) = harness();
    assert_eq!(
        selected(&h),
        at(16, 8, 15),
        "the newest capture starts selected"
    );

    h.state_mut()
        .as_mut()
        .unwrap()
        .jump_controls_mut()
        .set_target(date(14), 9, 10);
    h.run_steps(2);
    click(&mut h, GO_LABEL);
    assert_eq!(
        selected(&h),
        at(14, 9, 30),
        "between two captures: the later one"
    );
    assert_eq!(view(&h).jump_message(), None);

    // An exact match.
    h.state_mut()
        .as_mut()
        .unwrap()
        .jump_controls_mut()
        .set_target(date(12), 10, 30);
    h.run_steps(2);
    click(&mut h, GO_LABEL);
    assert_eq!(selected(&h), at(12, 10, 30));
    assert_eq!(view(&h).jump_message(), None);
}

#[test]
fn the_fields_follow_the_selection() {
    let (mut h, _) = harness();
    click(&mut h, TODAY_LABEL);
    assert_eq!(selected(&h), at(14, 9, 0));
    let c = h.state_mut().as_mut().unwrap().jump_controls_mut();
    assert_eq!((c.date(), c.hour(), c.minute()), (date(14), 9, 0));
    // An arrow key moves the selection, and the fields move with it.
    h.key_press(egui::Key::ArrowRight);
    h.run_steps(3);
    assert_eq!(selected(&h), at(14, 9, 30));
    let c = h.state_mut().as_mut().unwrap().jump_controls_mut();
    assert_eq!((c.date(), c.hour(), c.minute()), (date(14), 9, 30));
}

#[test]
fn each_quick_jump_button_selects_what_the_t18_query_returns() {
    for (kind, label) in QUICK_JUMPS {
        for start in captures() {
            let (mut h, _) = harness();
            h.state_mut().as_mut().unwrap().select(start);
            h.run_steps(3);
            assert_eq!(selected(&h), start);
            click(&mut h, label);
            assert_eq!(
                selected(&h),
                expected(kind, start),
                "{label} from {start:?}"
            );
        }
    }
}

#[test]
fn the_quick_jumps_land_where_expected() {
    let (mut h, _) = harness();
    // Today is May 14: its first capture.
    click(&mut h, TODAY_LABEL);
    assert_eq!(selected(&h), at(14, 9, 0));
    assert_eq!(view(&h).jump_message(), None);
    // Next day from May 14 is May 15 (empty): the next capture, May 16.
    click(&mut h, NEXT_DAY_LABEL);
    assert_eq!(selected(&h), at(16, 8, 15));
    // +1 h after the last capture falls back to the nearest (the last).
    click(&mut h, PLUS_HOUR_LABEL);
    assert_eq!(selected(&h), at(16, 8, 15));
    assert!(view(&h).jump_message().is_some());
    // -1 h from 08:15 on May 16 is 07:15: the last capture at or before it.
    click(&mut h, MINUS_HOUR_LABEL);
    assert_eq!(selected(&h), at(14, 12, 0));
    assert_eq!(view(&h).jump_message(), None);
    click(&mut h, PREVIOUS_DAY_LABEL);
    // May 13 is empty: the next capture is the first one on May 14.
    assert_eq!(selected(&h), at(14, 9, 0));
}

#[test]
fn jumping_to_an_empty_day_shows_the_showing_nearest_message() {
    let (mut h, _) = harness();
    // Yesterday (May 13) has no captures; the next one is on May 14.
    click(&mut h, YESTERDAY_LABEL);
    assert_eq!(selected(&h), at(14, 9, 0));
    let message = format!("No captures on 2025-05-13{SHOWING_NEAREST_SUFFIX}");
    assert_eq!(view(&h).jump_message(), Some(message.as_str()));
    assert!(h.query_by_label(&message).is_some(), "the message is shown");

    // Go to an empty day.
    h.state_mut()
        .as_mut()
        .unwrap()
        .jump_controls_mut()
        .set_target(date(15), 12, 0);
    h.run_steps(2);
    click(&mut h, GO_LABEL);
    assert_eq!(selected(&h), at(16, 8, 15));
    let message = format!("No captures on 2025-05-15{SHOWING_NEAREST_SUFFIX}");
    assert!(h.query_by_label(&message).is_some());

    // A jump that lands as asked clears the message.
    h.state_mut()
        .as_mut()
        .unwrap()
        .jump_controls_mut()
        .set_target(date(12), 10, 0);
    h.run_steps(2);
    click(&mut h, GO_LABEL);
    assert_eq!(selected(&h), at(12, 10, 0));
    assert!(h.query_by_label_contains("showing nearest").is_none());
}

#[test]
fn go_after_the_last_capture_of_a_day_with_captures_names_the_time() {
    let (mut h, _) = harness();
    h.state_mut()
        .as_mut()
        .unwrap()
        .jump_controls_mut()
        .set_target(date(12), 23, 30);
    h.run_steps(2);
    click(&mut h, GO_LABEL);
    // Nothing after 23:30 on May 12 until May 14.
    assert_eq!(selected(&h), at(14, 9, 0));
    let message = format!("No captures at or after 23:30 on 2025-05-12{SHOWING_NEAREST_SUFFIX}");
    assert_eq!(view(&h).jump_message(), Some(message.as_str()));
}

#[test]
fn after_a_jump_the_arrow_keys_continue_from_the_new_position() {
    let (mut h, _) = harness();
    click(&mut h, TODAY_LABEL);
    assert_eq!(selected(&h), at(14, 9, 0));
    h.key_press(egui::Key::ArrowRight);
    h.run_steps(3);
    assert_eq!(selected(&h), at(14, 9, 30));
    h.key_press(egui::Key::ArrowRight);
    h.run_steps(3);
    assert_eq!(selected(&h), at(14, 10, 30));
    h.key_press(egui::Key::ArrowLeft);
    h.run_steps(3);
    assert_eq!(selected(&h), at(14, 9, 30));
    h.key_press(egui::Key::ArrowLeft);
    h.key_press(egui::Key::ArrowLeft);
    h.run_steps(3);
    assert_eq!(selected(&h), at(12, 11, 0), "the capture before May 14");
}

#[test]
fn the_jump_target_is_loaded_first() {
    let (mut h, decoder) = harness();
    click(&mut h, PREVIOUS_DAY_LABEL);
    let target = selected(&h);
    wait_until(&mut h, "the target's full image", |_| {
        decoder
            .0
            .lock()
            .unwrap()
            .iter()
            .any(|&(k, t)| k == Kind::Full && t == target)
    });
}
