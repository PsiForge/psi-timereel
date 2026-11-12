//! egui_kittest tests of the set-up pages. They never reach the installation
//! step, so they touch no files.

mod common;

use common::*;
use egui_kittest::kittest::Queryable;
use timereel::setup_ui::{CONTINUE_LABEL, MISMATCH_MESSAGE, STORED_LABEL};

#[test]
fn mismatched_passwords_show_a_message_and_block_continue() {
    let mut h = harness(fast_options("true"));
    type_into(&mut h, 0, "correct horse battery");
    assert!(!has_label(&h, MISMATCH_MESSAGE));
    type_into(&mut h, 1, "correct horse batterz");
    assert!(has_label(&h, MISMATCH_MESSAGE));
    assert!(is_disabled(&h, CONTINUE_LABEL));
}

#[test]
fn a_short_password_warns_but_can_continue() {
    let mut h = harness(fast_options("true"));
    type_into(&mut h, 0, "abc");
    type_into(&mut h, 1, "abc");
    assert!(has_label_containing(&h, "shorter than 12 characters"));
    assert!(!has_label(&h, MISMATCH_MESSAGE));
    assert!(!is_disabled(&h, CONTINUE_LABEL));
    h.get_by_label(CONTINUE_LABEL).click();
    wait_until(&mut h, "recovery key page", |h| has_label(h, STORED_LABEL));
}

#[test]
fn a_long_password_has_no_warning() {
    let mut h = harness(fast_options("true"));
    type_into(&mut h, 0, "a long enough password");
    type_into(&mut h, 1, "a long enough password");
    assert!(!has_label_containing(&h, "shorter than 12 characters"));
    assert!(!is_disabled(&h, CONTINUE_LABEL));
}

#[test]
fn continue_stays_disabled_until_the_recovery_key_is_ticked() {
    let mut h = harness(fast_options("true"));
    submit_password(&mut h, "a long enough password");
    wait_until(&mut h, "recovery key page", |h| has_label(h, STORED_LABEL));
    assert!(is_disabled(&h, CONTINUE_LABEL));
    assert!(h.query_by_label_contains("Copy").is_none());
    h.get_by_label(STORED_LABEL).click();
    h.run_steps(2);
    assert!(!is_disabled(&h, CONTINUE_LABEL));
    // Unticking blocks it again.
    h.get_by_label(STORED_LABEL).click();
    h.run_steps(2);
    assert!(is_disabled(&h, CONTINUE_LABEL));
}
