#![allow(clippy::unwrap_used)]

use std::io;
use std::sync::Arc;

use age::x25519::Identity;
use timereel_capture::fake::{FakeLock, FakeScreen, FakeWindow};
use timereel_capture::{ActiveWindow, CaptureError};
use timereel_config::Config;
use timereel_store::{CaptureMeta, CaptureTime, Store, StoreError};

use super::*;

const T0: i64 = 1_747_215_000_123;
const NOW: Now = Now {
    unix_ms: T0,
    tz_offset_min: 120,
};

struct Setup {
    _dir: tempfile::TempDir,
    store: Store,
    id: Identity,
    config: Config,
}

fn setup() -> Setup {
    let dir = tempfile::tempdir().unwrap();
    let id = Identity::generate();
    let config = Config {
        public_key: Some(id.to_public().to_string()),
        ..Config::default()
    };
    Setup {
        store: Store::new(dir.path()),
        _dir: dir,
        id,
        config,
    }
}

fn runner_with(s: &Setup, window: FakeWindow, lock: FakeLock, screen: FakeScreen) -> CycleRunner {
    CycleRunner::new(
        Box::new(screen),
        Box::new(window),
        Box::new(lock),
        Box::new(Store::new(s.store.captures_dir().parent().unwrap())),
    )
}

fn runner(s: &Setup, window: FakeWindow) -> CycleRunner {
    runner_with(
        s,
        window,
        FakeLock::new(false),
        FakeScreen::single_monitor(),
    )
}

fn stored(s: &Setup) -> Vec<CaptureTime> {
    s.store.list(..).unwrap()
}

#[test]
fn normal_cycle_captures() {
    let s = setup();
    let mut r = runner(&s, FakeWindow::known("Firefox", "Inbox"));
    assert_eq!(
        r.run_once(&s.config, NOW),
        Outcome::Captured(CaptureTime(T0))
    );
    assert_eq!(stored(&s), vec![CaptureTime(T0)]);
    assert_eq!(r.state(), AgentState::Recording);
    assert_eq!(r.last_capture_ms(), T0);
}

#[test]
fn paused_skips_and_stores_nothing() {
    let s = setup();
    let mut config = s.config.clone();
    config.paused = true;
    let screen = FakeScreen::single_monitor();
    let mut r = runner_with(
        &s,
        FakeWindow::known("Firefox", "x"),
        FakeLock::new(false),
        screen,
    );
    assert_eq!(
        r.run_once(&config, NOW),
        Outcome::Skipped(SkipReason::Paused)
    );
    assert!(stored(&s).is_empty());
    assert_eq!(r.state(), AgentState::Paused);
    assert_eq!(r.last_skip(), Some(SkipReason::Paused));
    // Resuming returns to recording after the next cycle.
    assert!(matches!(r.run_once(&s.config, NOW), Outcome::Captured(_)));
    assert_eq!(r.state(), AgentState::Recording);
}

#[test]
fn locked_skips_and_stores_nothing() {
    let s = setup();
    let mut r = runner_with(
        &s,
        FakeWindow::known("Firefox", "x"),
        FakeLock::new(true),
        FakeScreen::single_monitor(),
    );
    assert_eq!(
        r.run_once(&s.config, NOW),
        Outcome::Skipped(SkipReason::Locked)
    );
    assert!(stored(&s).is_empty());
    assert_eq!(r.state(), AgentState::Recording);
}

#[test]
fn paused_wins_over_locked_and_nothing_is_asked() {
    let s = setup();
    let mut config = s.config.clone();
    config.paused = true;
    let mut r = runner_with(
        &s,
        FakeWindow::known("Firefox", "x"),
        FakeLock::new(true),
        FakeScreen::single_monitor(),
    );
    assert_eq!(
        r.run_once(&config, NOW),
        Outcome::Skipped(SkipReason::Paused)
    );
}

#[test]
fn excluded_skips_and_stores_nothing() {
    let s = setup();
    let mut config = s.config.clone();
    config.excluded_apps = vec!["KeePassXC".into()];
    let mut r = runner(&s, FakeWindow::known("KeePassXC", "Vault"));
    assert_eq!(
        r.run_once(&config, NOW),
        Outcome::Skipped(SkipReason::Excluded)
    );
    assert!(stored(&s).is_empty());
}

#[test]
fn excluded_check_happens_before_the_screen_is_captured() {
    let s = setup();
    let mut config = s.config.clone();
    config.excluded_apps = vec!["KeePassXC".into()];
    let screen = Arc::new(FakeScreen::single_monitor());
    struct Shared(Arc<FakeScreen>);
    impl timereel_capture::ScreenSource for Shared {
        fn capture(&self) -> Result<Vec<timereel_image::Frame>, CaptureError> {
            self.0.capture()
        }
    }
    let mut r = CycleRunner::new(
        Box::new(Shared(screen.clone())),
        Box::new(FakeWindow::known("KeePassXC", "Vault")),
        Box::new(FakeLock::new(false)),
        Box::new(Store::new(s.store.captures_dir().parent().unwrap())),
    );
    r.run_once(&config, NOW);
    assert_eq!(screen.calls(), 0);
}

#[test]
fn unknown_skips_by_default() {
    let s = setup();
    assert!(!s.config.capture_when_app_unknown);
    let mut r = runner(&s, FakeWindow::always(ActiveWindow::Unknown));
    assert_eq!(
        r.run_once(&s.config, NOW),
        Outcome::Skipped(SkipReason::AppUnknown)
    );
    assert!(stored(&s).is_empty());
}

#[test]
fn unknown_captures_when_allowed() {
    let s = setup();
    let mut config = s.config.clone();
    config.capture_when_app_unknown = true;
    let mut r = runner(&s, FakeWindow::always(ActiveWindow::Unknown));
    assert_eq!(r.run_once(&config, NOW), Outcome::Captured(CaptureTime(T0)));
    let meta = s.store.read_meta(&s.id, CaptureTime(T0)).unwrap();
    assert_eq!(meta.app_name, "unknown");
}

#[test]
fn no_window_captures_with_none_app() {
    let s = setup();
    let mut r = runner(&s, FakeWindow::always(ActiveWindow::NoWindow));
    assert_eq!(
        r.run_once(&s.config, NOW),
        Outcome::Captured(CaptureTime(T0))
    );
    let meta = s.store.read_meta(&s.id, CaptureTime(T0)).unwrap();
    assert_eq!(meta.app_name, "(none)");
}

#[test]
fn excluded_matching_is_case_insensitive_on_the_whole_name() {
    let list = vec!["KeePassXC".to_string(), "1Password".to_string()];
    assert!(is_excluded(&list, "keepassxc"));
    assert!(is_excluded(&list, "KEEPASSXC"));
    assert!(is_excluded(&list, "1password"));
    // Whole name only: no prefix, suffix or substring matches.
    assert!(!is_excluded(&list, "KeePass"));
    assert!(!is_excluded(&list, "KeePassXC Helper"));
    assert!(!is_excluded(&list, "xc"));
    assert!(!is_excluded(&[], "KeePassXC"));

    let s = setup();
    let mut config = s.config.clone();
    config.excluded_apps = vec!["KeePassXC".into()];
    let mut r = runner(&s, FakeWindow::known("keepassxc", "Vault"));
    assert_eq!(
        r.run_once(&config, NOW),
        Outcome::Skipped(SkipReason::Excluded)
    );
    let mut r = runner(&s, FakeWindow::known("KeePass", "Vault"));
    assert!(matches!(r.run_once(&config, NOW), Outcome::Captured(_)));
}

#[test]
fn excluded_app_active_during_capture_stores_nothing() {
    let s = setup();
    let mut config = s.config.clone();
    config.excluded_apps = vec!["KeePassXC".into()];
    let window = FakeWindow::sequence(vec![
        ActiveWindow::Known {
            app: "Firefox".into(),
            title: "Inbox".into(),
        },
        ActiveWindow::Known {
            app: "keepassxc".into(),
            title: "Vault".into(),
        },
    ]);
    let mut r = runner(&s, window);
    assert_eq!(
        r.run_once(&config, NOW),
        Outcome::Skipped(SkipReason::Excluded)
    );
    assert!(stored(&s).is_empty());
    assert_eq!(r.last_skip(), Some(SkipReason::Excluded));
}

#[test]
fn stored_metadata_has_timestamp_app_and_title() {
    let s = setup();
    let mut r = runner(&s, FakeWindow::known("Firefox", "Inbox - Mail"));
    let now = Now {
        unix_ms: T0 + 5,
        tz_offset_min: -90,
    };
    assert_eq!(
        r.run_once(&s.config, now),
        Outcome::Captured(CaptureTime(T0 + 5))
    );
    let meta = s.store.read_meta(&s.id, CaptureTime(T0 + 5)).unwrap();
    assert_eq!(
        meta,
        CaptureMeta {
            taken_at_ms: T0 + 5,
            tz_offset_min: -90,
            app_name: "Firefox".into(),
            window_title: "Inbox - Mail".into(),
            width: 64,
            height: 36,
            format: "webp".into(),
        }
    );
    // The image and thumbnail decode.
    let image = timereel_image::decode(&s.store.read_image(&s.id, CaptureTime(T0 + 5)).unwrap());
    assert_eq!((image.unwrap().width(), 36), (64, 36));
    assert!(
        timereel_image::decode(&s.store.read_thumb(&s.id, CaptureTime(T0 + 5)).unwrap()).is_ok()
    );
}

/// A sink that fails with a fixed error.
struct FailingSink(fn() -> StoreError);

impl CaptureSink for FailingSink {
    fn write(
        &self,
        _: &str,
        _: &CaptureMeta,
        _: &[u8],
        _: &[u8],
    ) -> Result<CaptureTime, StoreError> {
        Err((self.0)())
    }
}

fn io_error(code: i32) -> StoreError {
    StoreError::Io {
        action: "write",
        path: "/x".into(),
        source: io::Error::from_raw_os_error(code),
    }
}

fn failing_runner(
    window: FakeWindow,
    screen: FakeScreen,
    sink: Box<dyn CaptureSink>,
) -> CycleRunner {
    CycleRunner::new(
        Box::new(screen),
        Box::new(window),
        Box::new(FakeLock::new(false)),
        sink,
    )
}

#[test]
fn each_error_kind_sets_its_state_and_reason_and_success_recovers() {
    let s = setup();
    let ok_window = || FakeWindow::known("Firefox", "x");
    let cases: Vec<(ErrorKind, AgentState, &str, CycleRunner, Config)> = vec![
        (
            ErrorKind::NoDisplay,
            AgentState::Error,
            "No display found",
            failing_runner(
                ok_window(),
                FakeScreen::failing(CaptureError::NoDisplay),
                Box::new(Store::new(s.store.captures_dir().parent().unwrap())),
            ),
            s.config.clone(),
        ),
        (
            ErrorKind::NoPermission,
            AgentState::NeedsPermission,
            "Screen capture permission denied",
            failing_runner(
                ok_window(),
                FakeScreen::failing(CaptureError::NoPermission),
                Box::new(Store::new(s.store.captures_dir().parent().unwrap())),
            ),
            s.config.clone(),
        ),
        (
            ErrorKind::Portal,
            AgentState::Error,
            "Screen capture portal error",
            failing_runner(
                ok_window(),
                FakeScreen::failing(CaptureError::Portal),
                Box::new(Store::new(s.store.captures_dir().parent().unwrap())),
            ),
            s.config.clone(),
        ),
        (
            ErrorKind::DiskFull,
            AgentState::Error,
            "Disk full",
            failing_runner(
                ok_window(),
                FakeScreen::single_monitor(),
                Box::new(FailingSink(|| io_error(28))),
            ),
            s.config.clone(),
        ),
        (
            ErrorKind::Storage,
            AgentState::Error,
            "Could not save the screenshot",
            failing_runner(
                ok_window(),
                FakeScreen::single_monitor(),
                Box::new(FailingSink(|| io_error(13))),
            ),
            s.config.clone(),
        ),
        (
            // A public key that is not valid makes sealing fail.
            ErrorKind::Encryption,
            AgentState::Error,
            "Encryption failed",
            runner(&s, ok_window()),
            Config {
                public_key: Some("not-a-key".into()),
                ..s.config.clone()
            },
        ),
        (
            ErrorKind::NotSetUp,
            AgentState::NotSetUp,
            "No public key: run set-up in TimeReel",
            runner(&s, ok_window()),
            Config {
                public_key: None,
                ..s.config.clone()
            },
        ),
    ];
    for (kind, state, reason, mut r, config) in cases {
        assert_eq!(r.run_once(&config, NOW), Outcome::Failed(kind), "{kind:?}");
        assert_eq!(r.state(), state, "{kind:?}");
        assert_eq!(r.reason(), reason, "{kind:?}");
        // Fix the cause, then the next success returns to recording.
        r.screen = Box::new(FakeScreen::single_monitor());
        r.sink = Box::new(Store::new(s.store.captures_dir().parent().unwrap()));
        assert!(
            matches!(r.run_once(&s.config, NOW), Outcome::Captured(_)),
            "{kind:?}"
        );
        assert_eq!(r.state(), AgentState::Recording, "{kind:?}");
        assert_eq!(r.reason(), "", "{kind:?}");
        // Remove the capture so the next case can store at the same time.
        let path = s.store.path_of(CaptureTime(T0));
        std::fs::remove_file(path).unwrap();
    }
}

#[test]
fn error_state_survives_a_non_paused_skip() {
    let s = setup();
    let mut r = failing_runner(
        FakeWindow::known("Firefox", "x"),
        FakeScreen::failing(CaptureError::Portal),
        Box::new(Store::new(s.store.captures_dir().parent().unwrap())),
    );
    r.run_once(&s.config, NOW);
    r.lock = Box::new(FakeLock::new(true));
    assert_eq!(
        r.run_once(&s.config, NOW),
        Outcome::Skipped(SkipReason::Locked)
    );
    assert_eq!(r.state(), AgentState::Error);
    assert_eq!(r.reason(), "Screen capture portal error");
}

#[test]
fn store_errors_map_to_kinds() {
    assert_eq!(ErrorKind::from(&io_error(28)), ErrorKind::DiskFull);
    assert_eq!(ErrorKind::from(&io_error(5)), ErrorKind::Storage);
    assert_eq!(
        ErrorKind::from(&StoreError::AlreadyExists(CaptureTime(1))),
        ErrorKind::Storage
    );
    assert_eq!(
        ErrorKind::from(&StoreError::Container(ContainerError::Seal("x".into()))),
        ErrorKind::Encryption
    );
}
