#![allow(clippy::unwrap_used)]

use std::os::unix::fs::PermissionsExt;
use std::sync::{Mutex, MutexGuard};

use super::*;

/// Serialises tests that change process-wide environment variables and points
/// the three TimeReel directories at a fresh temp dir.
struct TestEnv {
    root: tempfile::TempDir,
    _guard: MutexGuard<'static, ()>,
}

static ENV_LOCK: Mutex<()> = Mutex::new(());

impl TestEnv {
    fn new() -> Self {
        let guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let root = tempfile::tempdir().expect("temp dir");
        // Nested, not yet existing, so tests also check directory creation.
        std::env::set_var(ENV_DATA_DIR, root.path().join("data/tr"));
        std::env::set_var(ENV_CONFIG_DIR, root.path().join("config/tr"));
        std::env::set_var(ENV_STATE_DIR, root.path().join("state/tr"));
        Self {
            root,
            _guard: guard,
        }
    }

    fn config_dir(&self) -> PathBuf {
        self.root.path().join("config/tr")
    }
}

fn mode(path: &Path) -> u32 {
    std::fs::metadata(path)
        .expect("metadata")
        .permissions()
        .mode()
        & 0o777
}

#[test]
fn defaults_match_architecture() {
    let c = Config::default();
    assert_eq!(c.interval_minutes, 5);
    assert_eq!(c.width_px, 1280);
    assert_eq!(c.format, ImageFormat::Webp);
    assert_eq!(c.quality, 50);
    assert_eq!(c.thumbnail_width_px, 240);
    assert_eq!(c.retention_days, 30);
    assert!(c.excluded_apps.is_empty());
    assert!(!c.capture_when_app_unknown);
    assert!(!c.paused);
    assert_eq!(c.ui_autolock_minutes, 10);
    assert_eq!(c.public_key, None);
    assert!(c.validate().is_ok());
}

#[test]
fn path_overrides_are_used() {
    let env = TestEnv::new();
    assert_eq!(data_dir().unwrap(), env.root.path().join("data/tr"));
    assert_eq!(config_dir().unwrap(), env.config_dir());
    assert_eq!(state_dir().unwrap(), env.root.path().join("state/tr"));
    assert_eq!(config_file().unwrap(), env.config_dir().join("config.toml"));
}

#[test]
fn xdg_variables_and_home_are_used_without_override() {
    let _env = TestEnv::new();
    std::env::remove_var(ENV_DATA_DIR);
    std::env::remove_var(ENV_CONFIG_DIR);
    std::env::set_var("XDG_DATA_HOME", "/xdg/data");
    std::env::set_var("XDG_CONFIG_HOME", "relative/is/ignored");
    std::env::remove_var(ENV_STATE_DIR);
    std::env::remove_var("XDG_STATE_HOME");
    std::env::set_var("HOME", "/home/someone");
    assert_eq!(data_dir().unwrap(), Path::new("/xdg/data/timereel"));
    assert_eq!(
        config_dir().unwrap(),
        Path::new("/home/someone/.config/timereel")
    );
    assert_eq!(
        state_dir().unwrap(),
        Path::new("/home/someone/.local/state/timereel")
    );
}

#[test]
fn each_out_of_range_value_gives_a_field_error() {
    type Change = fn(&mut Config);
    let cases: [(&str, Change, &str); 12] = [
        (
            "interval_minutes",
            |c| c.interval_minutes = 0,
            "between 1 and 60 minutes",
        ),
        (
            "interval_minutes",
            |c| c.interval_minutes = 61,
            "between 1 and 60 minutes",
        ),
        (
            "width_px",
            |c| c.width_px = 319,
            "between 320 and 7680 pixels",
        ),
        (
            "width_px",
            |c| c.width_px = 7681,
            "between 320 and 7680 pixels",
        ),
        ("quality", |c| c.quality = 0, "between 1 and 100"),
        ("quality", |c| c.quality = 101, "between 1 and 100"),
        (
            "thumbnail_width_px",
            |c| c.thumbnail_width_px = 119,
            "between 120 and 640 pixels",
        ),
        (
            "thumbnail_width_px",
            |c| c.thumbnail_width_px = 641,
            "between 120 and 640 pixels",
        ),
        (
            "retention_days",
            |c| c.retention_days = 0,
            "between 1 and 3650 days",
        ),
        (
            "retention_days",
            |c| c.retention_days = 3651,
            "between 1 and 3650 days",
        ),
        (
            "ui_autolock_minutes",
            |c| c.ui_autolock_minutes = 0,
            "between 1 and 240 minutes",
        ),
        (
            "ui_autolock_minutes",
            |c| c.ui_autolock_minutes = 241,
            "between 1 and 240 minutes",
        ),
    ];
    for (field, change, expected) in cases {
        let mut c = Config::default();
        change(&mut c);
        let errors = c.validate().expect_err(field);
        assert_eq!(errors.errors().len(), 1, "{field}: {errors}");
        let e = errors.for_field(field).expect(field);
        assert!(e.message.contains(expected), "{field}: {}", e.message);
    }
}

#[test]
fn range_boundaries_are_valid() {
    let c = Config {
        interval_minutes: 60,
        width_px: 320,
        quality: 1,
        thumbnail_width_px: 640,
        retention_days: 3650,
        ui_autolock_minutes: 240,
        ..Config::default()
    };
    assert!(c.validate().is_ok());
    let c = Config {
        interval_minutes: 1,
        width_px: 7680,
        quality: 100,
        thumbnail_width_px: 120,
        retention_days: 1,
        ui_autolock_minutes: 1,
        ..Config::default()
    };
    assert!(c.validate().is_ok());
}

#[test]
fn excluded_apps_rules() {
    let mut c = Config {
        excluded_apps: vec!["KeePassXC".into(), "  ".into(), " Bitwarden".into()],
        ..Config::default()
    };
    let errors = c.validate().expect_err("bad apps");
    assert!(errors.for_field("excluded_apps[0]").is_none());
    assert!(errors
        .for_field("excluded_apps[1]")
        .expect("empty")
        .message
        .contains("empty"));
    assert!(errors
        .for_field("excluded_apps[2]")
        .expect("untrimmed")
        .message
        .contains("spaces"));

    c.excluded_apps = (0..50).map(|i| format!("app{i}")).collect();
    assert!(c.validate().is_ok());
    c.excluded_apps.push("one-too-many".into());
    let errors = c.validate().expect_err("51 apps");
    assert!(errors
        .for_field("excluded_apps")
        .expect("count")
        .message
        .contains("At most 50"));
}

#[test]
fn all_errors_are_reported_together() {
    let c = Config {
        quality: 0,
        retention_days: 0,
        ..Config::default()
    };
    let errors = c.validate().expect_err("two errors");
    assert_eq!(errors.errors().len(), 2);
    let text = errors.to_string();
    assert!(
        text.contains("quality: ") && text.contains("retention_days: "),
        "{text}"
    );
}

#[test]
fn load_without_file_gives_defaults_and_creates_nothing() {
    let env = TestEnv::new();
    assert_eq!(load().unwrap(), Config::default());
    assert!(!env.config_dir().exists());
}

#[test]
fn load_reports_bad_files() {
    let env = TestEnv::new();
    std::fs::create_dir_all(env.config_dir()).unwrap();
    let path = env.config_dir().join("config.toml");

    std::fs::write(&path, "interval_minutes = \"five\"\n").unwrap();
    assert!(matches!(load(), Err(ConfigError::Parse { .. })));

    std::fs::write(&path, "format = 'png'\n").unwrap();
    assert!(matches!(load(), Err(ConfigError::Parse { .. })));

    std::fs::write(&path, "intervall_minutes = 5\n").unwrap();
    assert!(matches!(load(), Err(ConfigError::Parse { .. })));

    std::fs::write(&path, "quality = 500\n").unwrap();
    match load() {
        Err(ConfigError::Invalid(e)) => assert!(e.for_field("quality").is_some()),
        other => panic!("expected Invalid, got {other:?}"),
    }
}

#[test]
fn partial_file_uses_defaults_for_missing_keys() {
    let env = TestEnv::new();
    std::fs::create_dir_all(env.config_dir()).unwrap();
    std::fs::write(
        env.config_dir().join("config.toml"),
        "paused = true\npublic_key = 'age1abc'\nformat = 'jpeg'\n",
    )
    .unwrap();
    let c = load().unwrap();
    assert!(c.paused);
    assert_eq!(c.format, ImageFormat::Jpeg);
    assert_eq!(c.public_key.as_deref(), Some("age1abc"));
    assert_eq!(c.interval_minutes, 5);
}

#[test]
fn update_round_trips_and_leaves_no_temp_files() {
    let env = TestEnv::new();
    let saved = update(|c| {
        c.interval_minutes = 15;
        c.format = ImageFormat::Jpeg;
        c.excluded_apps = vec!["KeePassXC".into()];
        c.public_key = Some("age1xyz".into());
    })
    .unwrap();
    assert_eq!(load().unwrap(), saved);
    assert_eq!(saved.interval_minutes, 15);

    let mut names: Vec<_> = std::fs::read_dir(env.config_dir())
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    assert_eq!(names, ["config.lock", "config.toml"]);
}

#[test]
fn update_rejects_invalid_change_and_keeps_old_file() {
    let env = TestEnv::new();
    update(|c| c.quality = 70).unwrap();
    let path = env.config_dir().join("config.toml");
    let before = std::fs::read(&path).unwrap();

    match update(|c| c.quality = 0) {
        Err(ConfigError::Invalid(e)) => assert!(e.for_field("quality").is_some()),
        other => panic!("expected Invalid, got {other:?}"),
    }
    assert_eq!(std::fs::read(&path).unwrap(), before);
}

#[test]
fn two_threads_with_100_updates_each_lose_no_change() {
    let _env = TestEnv::new();
    update(|_| {}).unwrap();
    let a = std::thread::spawn(|| {
        for _ in 0..100 {
            update(|c| c.retention_days += 1).unwrap();
        }
    });
    let b = std::thread::spawn(|| {
        for _ in 0..100 {
            update(|c| c.width_px += 1).unwrap();
        }
    });
    a.join().unwrap();
    b.join().unwrap();
    let c = load().unwrap();
    assert_eq!(c.retention_days, 30 + 100);
    assert_eq!(c.width_px, 1280 + 100);
}

#[test]
fn failure_before_rename_leaves_old_config_intact() {
    let env = TestEnv::new();
    update(|c| c.interval_minutes = 7).unwrap();
    let path = env.config_dir().join("config.toml");
    let before = std::fs::read(&path).unwrap();

    let result = update_in(&env.config_dir(), |c| c.interval_minutes = 42, &|| {
        Err(io::Error::other("injected failure"))
    });
    match result {
        Err(ConfigError::Io { source, .. }) => assert!(source.to_string().contains("injected")),
        other => panic!("expected Io error, got {other:?}"),
    }
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert_eq!(load().unwrap().interval_minutes, 7);
    // The temp file was cleaned up and the lock was released.
    let names: Vec<_> = std::fs::read_dir(env.config_dir())
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    assert_eq!(names.len(), 2, "{names:?}");
    update(|c| c.interval_minutes = 8).unwrap();
}

#[test]
fn created_directories_are_0700_and_files_0600() {
    let env = TestEnv::new();
    update(|c| c.paused = true).unwrap();
    let dir = env.config_dir();
    assert_eq!(mode(&dir), 0o700);
    assert_eq!(mode(dir.parent().unwrap()), 0o700);
    assert_eq!(mode(&dir.join("config.toml")), 0o600);
    assert_eq!(mode(&dir.join("config.lock")), 0o600);

    let state = state_dir().unwrap();
    create_private_dir_all(&state).unwrap();
    assert_eq!(mode(&state), 0o700);
}

#[test]
fn existing_loose_directory_is_tightened() {
    let env = TestEnv::new();
    std::fs::create_dir_all(env.config_dir()).unwrap();
    std::fs::set_permissions(env.config_dir(), std::fs::Permissions::from_mode(0o755)).unwrap();
    update(|c| c.paused = true).unwrap();
    assert_eq!(mode(&env.config_dir()), 0o700);
}
