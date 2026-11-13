//! egui_kittest tests of the lock screen, the new-password offer, the Lock
//! button and auto-lock. Each test uses its own key file in a temp dir.

mod common;

use std::time::Duration;

use common::*;
use egui_kittest::kittest::Queryable;
use timereel::app::{LOCK_LABEL, TIMELINE_PLACEHOLDER};
use timereel::autolock::FakeClock;
use timereel::lock_ui::{
    NEW_PASSWORD_OFFER, NOT_NOW_LABEL, SET_PASSWORD_LABEL, UNLOCK_LABEL, WRONG_SECRET_MESSAGE,
};

const PASSWORD: &str = "correct horse battery";

#[test]
fn a_wrong_password_shows_the_generic_message_and_the_right_one_unlocks() {
    let dir = tempfile::tempdir().expect("temp dir");
    let (path, _) = make_keyring(dir.path(), PASSWORD);
    let mut h = viewer_harness(path, FakeClock::default());
    assert!(h.state().is_locked());

    unlock(&mut h, "not the password", false);
    wait_until(&mut h, "error message", |h| {
        has_label(h, WRONG_SECRET_MESSAGE)
    });
    assert!(h.state().is_locked());

    unlock(&mut h, PASSWORD, false);
    wait_until(&mut h, "timeline", |h| has_label(h, TIMELINE_PLACEHOLDER));
    assert!(h.state().is_unlocked());
    assert!(!has_label(&h, WRONG_SECRET_MESSAGE));
}

#[test]
fn a_wrong_recovery_key_shows_the_same_message() {
    let dir = tempfile::tempdir().expect("temp dir");
    let (path, _) = make_keyring(dir.path(), PASSWORD);
    let mut h = viewer_harness(path, FakeClock::default());
    unlock(&mut h, "AAAA-BBBB", true);
    wait_until(&mut h, "error message", |h| {
        has_label(h, WRONG_SECRET_MESSAGE)
    });
    assert!(h.state().is_locked());
}

#[test]
fn a_recovery_unlock_offers_a_new_password_that_then_unlocks() {
    let dir = tempfile::tempdir().expect("temp dir");
    let (path, recovery) = make_keyring(dir.path(), PASSWORD);
    let mut h = viewer_harness(path, FakeClock::default());

    unlock(&mut h, &recovery, true);
    wait_until(&mut h, "offer", |h| has_label(h, NEW_PASSWORD_OFFER));
    assert!(has_label(&h, NOT_NOW_LABEL));
    let new_password = "a brand new passphrase";
    type_into(&mut h, 0, new_password);
    type_into(&mut h, 1, new_password);
    h.get_by_label(SET_PASSWORD_LABEL).click();
    wait_until(&mut h, "timeline", |h| has_label(h, TIMELINE_PLACEHOLDER));

    h.get_by_label(LOCK_LABEL).click();
    h.run_steps(2);
    assert!(h.state().is_locked());

    unlock(&mut h, PASSWORD, false);
    wait_until(&mut h, "old password refused", |h| {
        has_label(h, WRONG_SECRET_MESSAGE)
    });
    unlock(&mut h, new_password, false);
    wait_until(&mut h, "timeline", |h| has_label(h, TIMELINE_PLACEHOLDER));
}

#[test]
fn not_now_skips_the_offer() {
    let dir = tempfile::tempdir().expect("temp dir");
    let (path, recovery) = make_keyring(dir.path(), PASSWORD);
    let mut h = viewer_harness(path, FakeClock::default());
    unlock(&mut h, &recovery, true);
    wait_until(&mut h, "offer", |h| has_label(h, NEW_PASSWORD_OFFER));
    h.get_by_label(NOT_NOW_LABEL).click();
    wait_until(&mut h, "timeline", |h| has_label(h, TIMELINE_PLACEHOLDER));
}

#[test]
fn no_input_for_the_autolock_time_locks_the_viewer() {
    let dir = tempfile::tempdir().expect("temp dir");
    let (path, _) = make_keyring(dir.path(), PASSWORD);
    let clock = FakeClock::default();
    let mut h = viewer_harness(path, clock.clone());
    unlock(&mut h, PASSWORD, false);
    wait_until(&mut h, "timeline", |h| has_label(h, TIMELINE_PLACEHOLDER));

    clock.advance(Duration::from_secs(9 * 60));
    h.run_steps(2);
    assert!(h.state().is_unlocked());
    clock.advance(Duration::from_secs(60));
    h.run_steps(2);
    assert!(h.state().is_locked());
    assert!(has_label(&h, UNLOCK_LABEL));
}
