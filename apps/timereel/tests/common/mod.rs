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
pub fn type_into<S>(h: &mut Harness<'static, S>, field: usize, text: &str) {
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

pub fn has_label<S>(h: &Harness<'static, S>, label: &str) -> bool {
    h.query_by_label(label).is_some()
}

pub fn has_label_containing(h: &Screen, part: &str) -> bool {
    h.query_by_label_contains(part).is_some()
}

pub fn is_disabled(h: &Screen, label: &str) -> bool {
    h.get_by_label(label).accesskit_node().is_disabled()
}

/// Steps the UI until `done` is true (workers run in other threads).
pub fn wait_until<S>(
    h: &mut Harness<'static, S>,
    what: &str,
    done: impl Fn(&Harness<'static, S>) -> bool,
) {
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

/// A key file with the password `password` in `dir`; returns its path and
/// the recovery key text.
pub fn make_keyring(dir: &std::path::Path, password: &str) -> (std::path::PathBuf, String) {
    use timereel_crypto::{create, save, ExposeSecret, SecretString};
    let (keyring, recovery, _) =
        create(&SecretString::from(password.to_owned()), FAST_KDF).expect("create");
    let path = dir.join(timereel::setup::KEYRING_FILE_NAME);
    save(&keyring, &path).expect("save");
    (path, recovery.display().expose_secret().to_owned())
}

pub type ViewerScreen = Harness<'static, timereel::app::Viewer>;

/// A harness around a viewer for the key file at `keyring_path`.
pub fn viewer_harness(
    keyring_path: std::path::PathBuf,
    clock: timereel::autolock::FakeClock,
) -> ViewerScreen {
    let options = timereel::app::ViewerOptions {
        keyring_path: keyring_path.clone(),
        autolock: timereel::autolock::autolock_after(10),
        clock: Box::new(clock),
        setup: fast_options("true"),
        timeline: timereel::timeline_ui::TimelineOptions {
            store: timereel_store::Store::new(keyring_path.parent().expect("data dir")),
            interval: Duration::from_secs(300),
        },
    };
    let mut h = Harness::builder()
        .with_size(egui::vec2(700.0, 600.0))
        .build_ui_state(
            |ui, viewer: &mut timereel::app::Viewer| viewer.ui(ui),
            timereel::app::Viewer::new(options),
        );
    h.run_steps(2);
    h
}

/// Types `secret` on the lock screen and presses Unlock.
pub fn unlock(h: &mut ViewerScreen, secret: &str, recovery: bool) {
    use timereel::lock_ui::{RECOVERY_TOGGLE_LABEL, UNLOCK_LABEL};
    if recovery {
        h.get_by_label(RECOVERY_TOGGLE_LABEL).click();
        h.run_steps(2);
    }
    type_into(h, 0, secret);
    h.get_by_label(UNLOCK_LABEL).click();
    h.run_steps(2);
}
