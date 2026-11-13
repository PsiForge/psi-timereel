//! Helpers shared by the Settings tests: an isolated config, data and key
//! directory, a fake agent that records its calls, and field helpers.
#![allow(dead_code)]

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Mutex, MutexGuard};

use egui_kittest::kittest::Queryable;
use egui_kittest::Harness;
use timereel::agent_client::{AgentControl, AgentError};
use timereel::settings::{SettingsOptions, SettingsScreen};
use timereel_crypto::KdfParams;
use timereel_store::{CaptureTime, Store};

/// Cheap Argon2 parameters so the tests run fast.
pub const FAST_KDF: KdfParams = KdfParams {
    m_kib: 64,
    t: 1,
    p: 1,
};

/// The config functions read the directories from the environment, which is
/// shared by all threads of a test program: tests take this lock.
static ENV_LOCK: Mutex<()> = Mutex::new(());

/// A private config, data and key directory for one test.
pub struct TestEnv {
    _guard: MutexGuard<'static, ()>,
    pub dir: tempfile::TempDir,
}

impl TestEnv {
    pub fn new() -> Self {
        let guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().expect("temp dir");
        std::env::set_var("TIMEREEL_CONFIG_DIR", dir.path().join("config"));
        std::env::set_var("TIMEREEL_DATA_DIR", dir.path().join("data"));
        std::env::set_var("TIMEREEL_STATE_DIR", dir.path().join("state"));
        Self { _guard: guard, dir }
    }

    pub fn config_file(&self) -> PathBuf {
        self.dir.path().join("config").join("config.toml")
    }

    pub fn store(&self) -> Store {
        Store::new(self.dir.path().join("data"))
    }

    pub fn keyring_path(&self) -> PathBuf {
        self.dir.path().join("data").join("keyring.json")
    }

    /// Creates a key file with `password`; returns the recovery key text.
    pub fn make_keyring(&self, password: &str) -> String {
        use timereel_crypto::{create, save, ExposeSecret, SecretString};
        let (keyring, recovery, _) =
            create(&SecretString::from(password.to_owned()), FAST_KDF).expect("create");
        save(&keyring, &self.keyring_path()).expect("save");
        recovery.display().expose_secret().to_owned()
    }

    /// Puts an (empty) capture file named for `time` in the store.
    pub fn add_capture(&self, time: CaptureTime) -> PathBuf {
        let path = self.store().path_of(time);
        std::fs::create_dir_all(path.parent().expect("day folder")).expect("day folder");
        std::fs::write(&path, b"").expect("capture");
        path
    }

    pub fn count_captures(&self) -> usize {
        fn walk(p: &Path) -> usize {
            std::fs::read_dir(p)
                .map(|it| {
                    it.flatten()
                        .map(|e| {
                            if e.path().is_dir() {
                                walk(&e.path())
                            } else {
                                1
                            }
                        })
                        .sum()
                })
                .unwrap_or(0)
        }
        walk(&self.dir.path().join("data").join("captures"))
    }
}

/// An agent double: records the calls and answers like a running or a
/// missing agent.
#[derive(Clone)]
pub struct FakeAgent {
    pub calls: Rc<RefCell<Vec<String>>>,
    pub running: bool,
    pub deleted: u32,
}

impl FakeAgent {
    pub fn running() -> Self {
        Self {
            calls: Rc::default(),
            running: true,
            deleted: 0,
        }
    }

    pub fn missing() -> Self {
        Self {
            running: false,
            ..Self::running()
        }
    }

    fn record(&self, call: &str) -> Result<(), AgentError> {
        self.calls.borrow_mut().push(call.to_owned());
        if self.running {
            Ok(())
        } else {
            Err(AgentError::NotRunning)
        }
    }
}

impl AgentControl for FakeAgent {
    fn reload_config(&self) -> Result<(), AgentError> {
        self.record("ReloadConfig")
    }

    fn run_retention(&self) -> Result<u32, AgentError> {
        self.record("RunRetention").map(|()| self.deleted)
    }

    fn set_paused(&self, paused: bool) -> Result<(), AgentError> {
        self.record(if paused { "Pause" } else { "Resume" })?;
        // Like the real agent, remember the state in the config.
        timereel_config::update(|c| c.paused = paused).expect("config update");
        Ok(())
    }
}

pub type SettingsHarness = Harness<'static, SettingsScreen>;

/// A harness around a Settings screen. `now` is the clock of the screen.
pub fn settings_harness(
    env: &TestEnv,
    agent: Box<dyn AgentControl>,
    now: CaptureTime,
) -> SettingsHarness {
    let screen = SettingsScreen::new(SettingsOptions {
        store: env.store(),
        keyring_path: env.keyring_path(),
        agent,
        now: Box::new(move || now),
    });
    let mut h = Harness::builder()
        .with_size(egui::vec2(800.0, 1000.0))
        .build_ui_state(
            |ui, screen: &mut SettingsScreen| {
                screen.ui(ui);
            },
            screen,
        );
    h.run_steps(2);
    h
}

/// Replaces the text of the single-line field number `index` (see
/// `timereel::settings::NUMBER_FIELDS` for the order).
pub fn set_field<S>(h: &mut Harness<'static, S>, index: usize, text: &str) {
    {
        let nodes = h.get_all_by_role(egui::accesskit::Role::TextInput);
        nodes.into_iter().nth(index).expect("field").focus();
    }
    h.run_steps(2);
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    h.run_steps(1);
    {
        let nodes = h.get_all_by_role(egui::accesskit::Role::TextInput);
        nodes.into_iter().nth(index).expect("field").type_text(text);
    }
    h.run_steps(2);
}

/// Types into password field `index`.
pub fn type_password<S>(h: &mut Harness<'static, S>, index: usize, text: &str) {
    {
        let nodes = h.get_all_by_role(egui::accesskit::Role::PasswordInput);
        nodes.into_iter().nth(index).expect("field").focus();
    }
    h.run_steps(2);
    {
        let nodes = h.get_all_by_role(egui::accesskit::Role::PasswordInput);
        nodes.into_iter().nth(index).expect("field").type_text(text);
    }
    h.run_steps(2);
}

pub fn has_text<S>(h: &Harness<'static, S>, part: &str) -> bool {
    h.query_by_label_contains(part).is_some()
}

/// Steps until `done` (workers run in other threads).
pub fn wait_until<S>(
    h: &mut Harness<'static, S>,
    what: &str,
    done: impl Fn(&Harness<'static, S>) -> bool,
) {
    let start = std::time::Instant::now();
    while !done(h) {
        assert!(
            start.elapsed() < std::time::Duration::from_secs(30),
            "timed out waiting for: {what}"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
        h.run_steps(1);
    }
}

pub fn read_config() -> timereel_config::Config {
    timereel_config::load().expect("config")
}

pub const DAY_MS: i64 = 86_400_000;
