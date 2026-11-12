//! Set-up end to end against throw-away directories. These tests change
//! process-wide environment variables, so they run one at a time.
#![allow(clippy::unwrap_used)]

mod common;

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock};

use common::*;
use egui_kittest::kittest::Queryable;
use tempfile::TempDir;
use timereel::setup::{self, SetupOptions};
use timereel::setup_ui::{CONTINUE_LABEL, STORED_LABEL};
use timereel_crypto::{ExposeSecret, SecretString};

static ENV_LOCK: Mutex<()> = Mutex::new(());
/// The real temp directory, read before any test points `TMPDIR` elsewhere.
static REAL_TMP: OnceLock<PathBuf> = OnceLock::new();

/// Points every TimeReel, cache and temp directory at one temp tree.
struct Sandbox {
    root: TempDir,
    _guard: MutexGuard<'static, ()>,
}

impl Sandbox {
    fn new() -> Self {
        let guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let real_tmp = REAL_TMP.get_or_init(std::env::temp_dir);
        let root = tempfile::tempdir_in(real_tmp).expect("temp dir");
        for name in ["data", "config", "state", "cache", "tmp"] {
            fs::create_dir(root.path().join(name)).expect("sub dir");
        }
        let at = |n: &str| root.path().join(n);
        std::env::set_var("TIMEREEL_DATA_DIR", at("data"));
        std::env::set_var("TIMEREEL_CONFIG_DIR", at("config"));
        std::env::set_var("TIMEREEL_STATE_DIR", at("state"));
        std::env::set_var("XDG_CACHE_HOME", at("cache"));
        std::env::set_var("TMPDIR", at("tmp"));
        Self {
            root,
            _guard: guard,
        }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.root.path().join(name)
    }

    /// A fake `systemctl` that records its arguments and exits with `code`.
    fn fake_systemctl(&self, code: i32, message: &str) -> (String, PathBuf) {
        let log = self.path("systemctl.log");
        let script = self.path("fake-systemctl");
        fs::write(
            &script,
            format!(
                "#!/bin/sh\necho \"$@\" > '{}'\necho '{message}' >&2\nexit {code}\n",
                log.display()
            ),
        )
        .expect("script");
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).expect("chmod");
        (script.to_string_lossy().into_owned(), log)
    }
}

fn mode(path: &Path) -> u32 {
    fs::metadata(path).expect("metadata").permissions().mode() & 0o777
}

/// Every file below `dir` that contains one of `needles`.
fn files_containing(dir: &Path, needles: &[Vec<u8>]) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for entry in fs::read_dir(&d).expect("read_dir").flatten() {
            let path = entry.path();
            let kind = entry.file_type().expect("file type");
            if kind.is_dir() {
                stack.push(path);
            } else if kind.is_file() {
                let bytes = fs::read(&path).expect("read");
                if needles
                    .iter()
                    .any(|n| bytes.windows(n.len()).any(|w| w == n.as_slice()))
                {
                    found.push(path);
                }
            }
        }
    }
    found
}

/// The recovery key as it may appear in a file: grouped, joined and spaced.
fn needles(recovery: &SecretString) -> Vec<Vec<u8>> {
    let grouped = recovery.expose_secret().to_owned();
    let joined = grouped.replace('-', "");
    let spaced = grouped.replace('-', " ");
    // The part after "trk1-" is the secret; look for it alone, too.
    let body = joined.trim_start_matches("trk1").to_owned();
    [grouped, joined, spaced, body]
        .into_iter()
        .map(String::into_bytes)
        .collect()
}

fn new_key(options: &SetupOptions) -> setup::NewKey {
    setup::create_key(
        &SecretString::from("a long enough password".to_owned()),
        options,
    )
    .expect("create key")
}

#[test]
fn set_up_stores_the_public_key_and_a_private_key_file() {
    let sb = Sandbox::new();
    let (systemctl, log) = sb.fake_systemctl(0, "");
    let options = fast_options(&systemctl);
    assert!(!setup::is_set_up().unwrap());

    let key = new_key(&options);
    setup::install(key.keyring()).expect("install");
    setup::enable_agent(&options).expect("enable");

    let keyring_file = sb.path("data").join("keyring.json");
    assert!(setup::is_set_up().unwrap());
    assert_eq!(mode(&keyring_file), 0o600);
    let config = timereel_config::load().expect("config");
    assert_eq!(
        config.public_key.as_deref(),
        Some(key.keyring().public_key())
    );
    assert_eq!(mode(&sb.path("config").join("config.toml")), 0o600);
    assert_eq!(
        fs::read_to_string(log).unwrap().trim(),
        "--user enable --now timereel-agent.service"
    );
}

#[test]
fn install_never_replaces_an_existing_key_file() {
    let sb = Sandbox::new();
    let options = fast_options("true");
    let first = new_key(&options);
    setup::install(first.keyring()).expect("install");
    // Same key again (a retry after a failed step) is fine.
    setup::install(first.keyring()).expect("repeat");
    let second = new_key(&options);
    let err = setup::install(second.keyring()).expect_err("must refuse");
    assert!(err.to_string().contains("already exists"));
    let kept = timereel_crypto::load(&sb.path("data").join("keyring.json")).unwrap();
    assert_eq!(kept.public_key(), first.keyring().public_key());
}

#[test]
fn the_recovery_key_is_written_to_no_file() {
    let sb = Sandbox::new();
    let (systemctl, _log) = sb.fake_systemctl(0, "");
    let options = fast_options(&systemctl);
    let key = new_key(&options);
    let recovery = key.recovery_text();
    let needles = needles(&recovery);

    // The scan itself works: a planted copy is found.
    let planted = sb.path("tmp").join("planted");
    fs::write(&planted, recovery.expose_secret()).unwrap();
    assert_eq!(
        files_containing(&sb.path("tmp"), &needles),
        std::slice::from_ref(&planted)
    );
    fs::remove_file(&planted).unwrap();

    setup::install(key.keyring()).expect("install");
    setup::enable_agent(&options).expect("enable");
    for dir in ["data", "config", "state", "cache", "tmp"] {
        let hits = files_containing(&sb.path(dir), &needles);
        assert!(hits.is_empty(), "recovery key found in {hits:?}");
    }
    // The key file still holds the slot that the recovery key opens.
    let keyring = timereel_crypto::load(&sb.path("data").join("keyring.json")).unwrap();
    timereel_crypto::unlock_with_recovery_key(&keyring, &recovery).expect("recovery key works");
}

#[test]
fn a_failing_systemctl_shows_an_error_and_does_not_crash() {
    let sb = Sandbox::new();
    let (systemctl, log) = sb.fake_systemctl(1, "Failed to connect to bus");
    let mut h = harness(fast_options(&systemctl));
    submit_password(&mut h, "a long enough password");
    wait_until(&mut h, "recovery key page", |h| has_label(h, STORED_LABEL));
    h.get_by_label(STORED_LABEL).click();
    h.run_steps(2);
    h.get_by_label(CONTINUE_LABEL).click();
    wait_until(&mut h, "service error", |h| {
        has_label_containing(h, "could not be started")
    });
    assert!(has_label_containing(&h, "exited with code 1"));
    assert!(has_label_containing(&h, "Failed to connect to bus"));
    assert!(has_label(&h, "Try again"));
    assert!(fs::read_to_string(log).unwrap().contains("--now"));
    // The key and the config were saved anyway.
    assert!(sb.path("data").join("keyring.json").exists());
    assert!(timereel_config::load().unwrap().public_key.is_some());
    // The user can still go on.
    h.get_by_label(CONTINUE_LABEL).click();
    h.run_steps(2);
}

#[test]
fn a_missing_systemctl_is_an_error_message() {
    let sb = Sandbox::new();
    let missing = sb.path("no-such-program").to_string_lossy().into_owned();
    let err = setup::enable_agent(&fast_options(&missing)).expect_err("must fail");
    assert!(err.starts_with("Could not run systemctl"), "{err}");
}

#[test]
fn a_successful_set_up_through_the_ui_ends_without_an_error() {
    let sb = Sandbox::new();
    let (systemctl, _log) = sb.fake_systemctl(0, "");
    let mut h = harness(fast_options(&systemctl));
    submit_password(&mut h, "a long enough password");
    wait_until(&mut h, "recovery key page", |h| has_label(h, STORED_LABEL));
    h.get_by_label(STORED_LABEL).click();
    h.run_steps(2);
    h.get_by_label(CONTINUE_LABEL).click();
    wait_until(&mut h, "done page", |h| {
        has_label_containing(h, "is set up and has started")
    });
    assert!(!has_label_containing(&h, "could not be started"));
    assert!(sb.path("data").join("keyring.json").exists());
}
