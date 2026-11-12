//! TimeReel configuration: XDG paths, the `config.toml` schema, validation
//! and an atomic, file-locked read-modify-write (architecture section 3.2).
//!
//! The viewer and the agent change the config only through [`update`]. Readers
//! use [`load`]; they need no lock because the file is always replaced by an
//! atomic rename.
//!
//! Directories are created with mode 0700 and files with mode 0600.

mod fsutil;
mod paths;

use std::fmt;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use fs4::FileExt;
use serde::{Deserialize, Serialize};

pub use fsutil::{create_private_dir_all, create_private_new, sync_dir, DIR_MODE, FILE_MODE};
pub use paths::{
    config_dir, config_file, data_dir, state_dir, PathError, ENV_CONFIG_DIR, ENV_DATA_DIR,
    ENV_STATE_DIR,
};

/// File name of the config inside the config directory.
pub const CONFIG_FILE_NAME: &str = "config.toml";
/// File name of the lock file inside the config directory.
pub const LOCK_FILE_NAME: &str = "config.lock";

/// Allowed range for `interval_minutes`.
pub const INTERVAL_MINUTES_RANGE: std::ops::RangeInclusive<u32> = 1..=60;
/// Allowed range for `width_px`.
pub const WIDTH_PX_RANGE: std::ops::RangeInclusive<u32> = 320..=7680;
/// Allowed range for `quality`.
pub const QUALITY_RANGE: std::ops::RangeInclusive<u32> = 1..=100;
/// Allowed range for `thumbnail_width_px`.
pub const THUMBNAIL_WIDTH_PX_RANGE: std::ops::RangeInclusive<u32> = 120..=640;
/// Allowed range for `retention_days`.
pub const RETENTION_DAYS_RANGE: std::ops::RangeInclusive<u32> = 1..=3650;
/// Allowed range for `ui_autolock_minutes`.
pub const UI_AUTOLOCK_MINUTES_RANGE: std::ops::RangeInclusive<u32> = 1..=240;
/// Maximum number of entries in `excluded_apps`.
pub const MAX_EXCLUDED_APPS: usize = 50;

/// Image format of stored captures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ImageFormat {
    /// WebP (the default, ADR 0011).
    Webp,
    /// JPEG.
    Jpeg,
}

/// The contents of `config.toml`. Missing keys take their default value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// Minutes between captures (1-60).
    pub interval_minutes: u32,
    /// Width of the stored image in pixels (320-7680).
    pub width_px: u32,
    /// Image format of stored captures.
    pub format: ImageFormat,
    /// Image quality (1-100).
    pub quality: u32,
    /// Width of the thumbnail in pixels (120-640).
    pub thumbnail_width_px: u32,
    /// Days to keep captures (1-3650).
    pub retention_days: u32,
    /// App names that are never captured: trimmed, non-empty, at most 50.
    pub excluded_apps: Vec<String>,
    /// Capture even if the active app cannot be determined.
    pub capture_when_app_unknown: bool,
    /// Recording is paused.
    pub paused: bool,
    /// Minutes of idle time before the viewer locks itself (1-240).
    pub ui_autolock_minutes: u32,
    /// The age public key (`age1...`); `None` until set-up has run.
    pub public_key: Option<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            interval_minutes: 5,
            width_px: 1280,
            format: ImageFormat::Webp,
            quality: 50,
            thumbnail_width_px: 240,
            retention_days: 30,
            excluded_apps: Vec::new(),
            capture_when_app_unknown: false,
            paused: false,
            ui_autolock_minutes: 10,
            public_key: None,
        }
    }
}

/// A problem with one config field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldError {
    /// The field name as written in `config.toml`, for example
    /// `quality` or `excluded_apps[2]`.
    pub field: String,
    /// A message that can be shown to the user as it is.
    pub message: String,
}

impl fmt::Display for FieldError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.field, self.message)
    }
}

/// All field errors found by [`Config::validate`]. Never empty.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationErrors(pub Vec<FieldError>);

impl ValidationErrors {
    /// The errors, one per problem.
    pub fn errors(&self) -> &[FieldError] {
        &self.0
    }

    /// The first error for `field`, if any.
    pub fn for_field(&self, field: &str) -> Option<&FieldError> {
        self.0.iter().find(|e| e.field == field)
    }
}

impl fmt::Display for ValidationErrors {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, e) in self.0.iter().enumerate() {
            if i > 0 {
                f.write_str("; ")?;
            }
            write!(f, "{e}")?;
        }
        Ok(())
    }
}

impl std::error::Error for ValidationErrors {}

/// Why reading or updating the config failed.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// A directory could not be determined.
    #[error(transparent)]
    Path(#[from] PathError),
    /// A file operation failed.
    #[error("could not {action} {}: {source}", path.display())]
    Io {
        /// What was being done, for example `read` or `lock`.
        action: &'static str,
        /// The file or directory involved.
        path: PathBuf,
        /// The underlying error.
        #[source]
        source: io::Error,
    },
    /// The file is not valid TOML for this schema.
    #[error("config file {} is not valid: {message}", path.display())]
    Parse {
        /// The config file.
        path: PathBuf,
        /// The parser's message, including the line.
        message: String,
    },
    /// The values are out of range.
    #[error("invalid config: {0}")]
    Invalid(#[from] ValidationErrors),
}

impl ConfigError {
    fn io(action: &'static str, path: &Path, source: io::Error) -> Self {
        Self::Io {
            action,
            path: path.to_path_buf(),
            source,
        }
    }
}

fn range_error<T: PartialOrd + fmt::Display + Copy>(
    errors: &mut Vec<FieldError>,
    field: &str,
    label: &str,
    unit: &str,
    value: T,
    range: &std::ops::RangeInclusive<T>,
) {
    if !range.contains(&value) {
        errors.push(FieldError {
            field: field.to_string(),
            message: format!(
                "{label} must be between {} and {}{unit}, but is {value}",
                range.start(),
                range.end()
            ),
        });
    }
}

impl Config {
    /// Checks every field and returns all problems, not only the first.
    pub fn validate(&self) -> Result<(), ValidationErrors> {
        let mut errors = Vec::new();
        range_error(
            &mut errors,
            "interval_minutes",
            "The capture interval",
            " minutes",
            self.interval_minutes,
            &INTERVAL_MINUTES_RANGE,
        );
        range_error(
            &mut errors,
            "width_px",
            "The image width",
            " pixels",
            self.width_px,
            &WIDTH_PX_RANGE,
        );
        range_error(
            &mut errors,
            "quality",
            "The image quality",
            "",
            self.quality,
            &QUALITY_RANGE,
        );
        range_error(
            &mut errors,
            "thumbnail_width_px",
            "The thumbnail width",
            " pixels",
            self.thumbnail_width_px,
            &THUMBNAIL_WIDTH_PX_RANGE,
        );
        range_error(
            &mut errors,
            "retention_days",
            "The retention time",
            " days",
            self.retention_days,
            &RETENTION_DAYS_RANGE,
        );
        range_error(
            &mut errors,
            "ui_autolock_minutes",
            "The auto-lock time",
            " minutes",
            self.ui_autolock_minutes,
            &UI_AUTOLOCK_MINUTES_RANGE,
        );
        if self.excluded_apps.len() > MAX_EXCLUDED_APPS {
            errors.push(FieldError {
                field: "excluded_apps".to_string(),
                message: format!(
                    "At most {MAX_EXCLUDED_APPS} excluded apps are allowed, but there are {}",
                    self.excluded_apps.len()
                ),
            });
        }
        for (i, app) in self.excluded_apps.iter().enumerate() {
            let message = if app.trim().is_empty() {
                Some("An excluded app name must not be empty")
            } else if app.trim() != app {
                Some("An excluded app name must not start or end with spaces")
            } else {
                None
            };
            if let Some(message) = message {
                errors.push(FieldError {
                    field: format!("excluded_apps[{i}]"),
                    message: message.to_string(),
                });
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(ValidationErrors(errors))
        }
    }
}

/// Parses and validates the config file at `path`; a missing file gives the
/// defaults.
fn load_file(path: &Path) -> Result<Config, ConfigError> {
    let mut text = String::new();
    match std::fs::File::open(path) {
        Ok(mut file) => {
            file.read_to_string(&mut text)
                .map_err(|e| ConfigError::io("read", path, e))?;
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Config::default()),
        Err(e) => return Err(ConfigError::io("open", path, e)),
    }
    let config: Config = toml::from_str(&text).map_err(|e| ConfigError::Parse {
        path: path.to_path_buf(),
        message: e.to_string(),
    })?;
    config.validate()?;
    Ok(config)
}

/// Reads the config from the config directory.
///
/// If the file does not exist this returns the defaults, with no public key.
/// A file that is malformed or out of range is an error, so that a broken
/// edit is reported rather than silently replaced by defaults.
pub fn load() -> Result<Config, ConfigError> {
    load_file(&config_file()?)
}

/// Atomically changes the config.
///
/// Takes an exclusive `flock` on `config.lock`, reads the current config
/// (the defaults if there is none), applies `change`, validates the result,
/// writes it to a temp file in the same directory, fsyncs it, renames it over
/// `config.toml` and fsyncs the directory. If anything fails, the old
/// `config.toml` is left as it was. Returns the config that was saved.
pub fn update<F>(change: F) -> Result<Config, ConfigError>
where
    F: FnOnce(&mut Config),
{
    update_in(&config_dir()?, change, &|| Ok(()))
}

/// Counter that keeps temp file names unique inside one process.
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Implementation of [`update`] for an explicit directory. `before_rename`
/// runs after the temp file is on disk and before it is renamed; tests use it
/// to inject a failure.
fn update_in<F>(
    dir: &Path,
    change: F,
    before_rename: &dyn Fn() -> io::Result<()>,
) -> Result<Config, ConfigError>
where
    F: FnOnce(&mut Config),
{
    create_private_dir_all(dir).map_err(|e| ConfigError::io("create directory", dir, e))?;

    let lock_path = dir.join(LOCK_FILE_NAME);
    let lock = fsutil::open_private_rw(&lock_path)
        .map_err(|e| ConfigError::io("open lock file", &lock_path, e))?;
    FileExt::lock(&lock).map_err(|e| ConfigError::io("lock", &lock_path, e))?;
    // The lock is released when `lock` is dropped, on every return path.

    let config_path = dir.join(CONFIG_FILE_NAME);
    let mut config = load_file(&config_path)?;
    change(&mut config);
    config.validate()?;
    let text = toml::to_string_pretty(&config).map_err(|e| ConfigError::Parse {
        path: config_path.clone(),
        message: e.to_string(),
    })?;

    let temp_path = dir.join(format!(
        ".{CONFIG_FILE_NAME}.{}.{}.tmp",
        std::process::id(),
        TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let result = write_and_replace(&temp_path, &config_path, dir, &text, before_rename);
    if result.is_err() {
        // Best effort: the temp file may not exist, and the original error
        // is the one worth reporting.
        let _ = std::fs::remove_file(&temp_path);
    }
    result?;
    Ok(config)
}

fn write_and_replace(
    temp_path: &Path,
    config_path: &Path,
    dir: &Path,
    text: &str,
    before_rename: &dyn Fn() -> io::Result<()>,
) -> Result<(), ConfigError> {
    let mut file = fsutil::create_private_new(temp_path)
        .map_err(|e| ConfigError::io("create", temp_path, e))?;
    file.write_all(text.as_bytes())
        .map_err(|e| ConfigError::io("write", temp_path, e))?;
    file.sync_all()
        .map_err(|e| ConfigError::io("sync", temp_path, e))?;
    drop(file);
    before_rename().map_err(|e| ConfigError::io("replace", config_path, e))?;
    std::fs::rename(temp_path, config_path)
        .map_err(|e| ConfigError::io("replace", config_path, e))?;
    fsutil::sync_dir(dir).map_err(|e| ConfigError::io("sync directory", dir, e))
}

#[cfg(test)]
mod tests;
