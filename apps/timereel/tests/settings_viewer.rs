//! The Settings screen inside the viewer: reachable only while unlocked, a
//! saved auto-lock time applies at once, and locking closes the screen.

mod common;
mod settings_support;

use std::time::Duration;

use common::*;
use egui_kittest::kittest::Queryable;
use timereel::app::LOCK_LABEL;
use timereel::autolock::FakeClock;
use timereel::lock_ui::UNLOCK_LABEL;
use timereel::settings::*;
use timereel::timeline_ui::EMPTY_TIMELINE_MESSAGE;

#[test]
fn settings_open_from_the_unlocked_view_and_the_new_autolock_time_applies() {
    let env = settings_support::TestEnv::new();
    let password = "correct horse battery";
    env.make_keyring(password);
    let clock = FakeClock::default();
    let mut h = viewer_harness(env.keyring_path(), clock.clone());
    unlock(&mut h, password, false);
    wait_until(&mut h, "timeline", |h| has_label(h, EMPTY_TIMELINE_MESSAGE));

    h.get_by_label(SETTINGS_LABEL).click();
    h.run_steps(2);
    assert!(h.state().settings_open());
    assert!(has_label(&h, SETTINGS_HEADING));

    settings_support::set_field(&mut h, 5, "2");
    h.get_by_label(SAVE_LABEL).click();
    h.run_steps(2);
    assert_eq!(settings_support::read_config().ui_autolock_minutes, 2);

    // Two minutes of silence now lock the viewer (the default was ten).
    clock.advance(Duration::from_secs(121));
    h.run_steps(2);
    assert!(h.state().is_locked());
    assert!(!h.state().settings_open());
    assert!(has_label(&h, UNLOCK_LABEL));
}

#[test]
fn back_returns_to_the_main_view_and_lock_still_works() {
    let env = settings_support::TestEnv::new();
    let password = "correct horse battery";
    env.make_keyring(password);
    let mut h = viewer_harness(env.keyring_path(), FakeClock::default());
    unlock(&mut h, password, false);
    wait_until(&mut h, "timeline", |h| has_label(h, EMPTY_TIMELINE_MESSAGE));
    h.get_by_label(SETTINGS_LABEL).click();
    h.run_steps(2);
    h.get_by_label(BACK_LABEL).click();
    h.run_steps(2);
    assert!(!h.state().settings_open());
    assert!(has_label(&h, EMPTY_TIMELINE_MESSAGE));

    h.get_by_label(SETTINGS_LABEL).click();
    h.run_steps(2);
    h.get_by_label(LOCK_LABEL).click();
    h.run_steps(2);
    assert!(h.state().is_locked());
    assert!(!h.state().settings_open());
}
