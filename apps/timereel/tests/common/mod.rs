//! Helpers shared by the set-up tests.
#![allow(dead_code)]

use std::time::{Duration, Instant};

use egui_kittest::kittest::{NodeT, Queryable};
use egui_kittest::Harness;
use timereel::setup::SetupOptions;
use timereel::setup_ui::SetupScreen;
use timereel_crypto::KdfParams;

/// Cheap Argon2 parameters so the tests run fast.
pub const FAST_KDF: KdfParams = KdfParams {
    m_kib: 64,
    t: 1,
    p: 1,
};

pub fn fast_options(systemctl: &str) -> SetupOptions {
    SetupOptions {
        kdf: FAST_KDF,
        systemctl: systemctl.into(),
    }
}

pub type Screen = Harness<'static>;

/// A harness that shows a set-up screen.
pub fn harness(options: SetupOptions) -> Screen {
    let mut screen = SetupScreen::new(options);
    let mut h = Harness::builder()
        .with_size(egui::vec2(700.0, 600.0))
        .build_ui(move |ui| {
            screen.ui(ui);
        });
    h.run_steps(2);
    h
}

/// Types into the password field (0) or the confirmation field (1).
pub fn type_into(h: &mut Screen, field: usize, text: &str) {
    {
        let nodes = h.get_all_by_role(egui::accesskit::Role::PasswordInput);
        let node = nodes.into_iter().nth(field).expect("password field");
        node.focus();
    }
    h.run_steps(2);
    {
        let nodes = h.get_all_by_role(egui::accesskit::Role::PasswordInput);
        nodes.into_iter().nth(field).expect("field").type_text(text);
    }
    h.run_steps(2);
}

pub fn has_label(h: &Screen, label: &str) -> bool {
    h.query_by_label(label).is_some()
}

pub fn has_label_containing(h: &Screen, part: &str) -> bool {
    h.query_by_label_contains(part).is_some()
}

pub fn is_disabled(h: &Screen, label: &str) -> bool {
    h.get_by_label(label).accesskit_node().is_disabled()
}

/// Steps the UI until `done` is true (workers run in other threads).
pub fn wait_until(h: &mut Screen, what: &str, done: impl Fn(&Screen) -> bool) {
    let start = Instant::now();
    while !done(h) {
        assert!(
            start.elapsed() < Duration::from_secs(30),
            "timed out waiting for: {what}"
        );
        std::thread::sleep(Duration::from_millis(20));
        h.run_steps(1);
    }
}

/// Fills in a matching password and presses Continue.
pub fn submit_password(h: &mut Screen, password: &str) {
    type_into(h, 0, password);
    type_into(h, 1, password);
    h.get_by_label(timereel::setup_ui::CONTINUE_LABEL).click();
    h.run_steps(2);
}
