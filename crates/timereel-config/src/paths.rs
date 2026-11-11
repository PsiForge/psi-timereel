//! XDG directory resolution with test overrides.

use std::path::PathBuf;

/// Environment variable that overrides the data directory.
pub const ENV_DATA_DIR: &str = "TIMEREEL_DATA_DIR";
/// Environment variable that overrides the config directory.
pub const ENV_CONFIG_DIR: &str = "TIMEREEL_CONFIG_DIR";
/// Environment variable that overrides the state directory.
pub const ENV_STATE_DIR: &str = "TIMEREEL_STATE_DIR";

/// Name of the application sub-directory below each XDG base directory.
const APP_DIR: &str = "timereel";

/// Why a directory could not be determined.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PathError {
    /// Neither the override, the XDG variable nor `HOME` is usable.
    #[error(
        "cannot find the {kind} directory: set {override_var}, {xdg_var} or HOME to an absolute path"
    )]
    NoBase {
        /// Which directory was asked for (`data`, `config` or `state`).
        kind: &'static str,
        /// The TimeReel override variable.
        override_var: &'static str,
        /// The XDG variable.
        xdg_var: &'static str,
    },
}

/// Reads an environment variable, accepting only a non-empty absolute path
/// (the XDG specification says relative paths must be ignored).
fn absolute_env(name: &str) -> Option<PathBuf> {
    let value = std::env::var_os(name)?;
    let path = PathBuf::from(value);
    path.is_absolute().then_some(path)
}

fn resolve(
    kind: &'static str,
    override_var: &'static str,
    xdg_var: &'static str,
    home_fallback: &[&str],
) -> Result<PathBuf, PathError> {
    // An override names the TimeReel directory itself, so no suffix is added.
    if let Some(dir) = absolute_env(override_var) {
        return Ok(dir);
    }
    if let Some(base) = absolute_env(xdg_var) {
        return Ok(base.join(APP_DIR));
    }
    if let Some(mut home) = absolute_env("HOME") {
        home.extend(home_fallback);
        return Ok(home.join(APP_DIR));
    }
    Err(PathError::NoBase {
        kind,
        override_var,
        xdg_var,
    })
}

/// Data directory: `$TIMEREEL_DATA_DIR`, else `$XDG_DATA_HOME/timereel`,
/// else `~/.local/share/timereel`.
pub fn data_dir() -> Result<PathBuf, PathError> {
    resolve("data", ENV_DATA_DIR, "XDG_DATA_HOME", &[".local", "share"])
}

/// Config directory: `$TIMEREEL_CONFIG_DIR`, else `$XDG_CONFIG_HOME/timereel`,
/// else `~/.config/timereel`.
pub fn config_dir() -> Result<PathBuf, PathError> {
    resolve("config", ENV_CONFIG_DIR, "XDG_CONFIG_HOME", &[".config"])
}

/// State directory: `$TIMEREEL_STATE_DIR`, else `$XDG_STATE_HOME/timereel`,
/// else `~/.local/state/timereel`.
pub fn state_dir() -> Result<PathBuf, PathError> {
    resolve(
        "state",
        ENV_STATE_DIR,
        "XDG_STATE_HOME",
        &[".local", "state"],
    )
}

/// Path of `config.toml` inside the config directory.
pub fn config_file() -> Result<PathBuf, PathError> {
    Ok(config_dir()?.join(crate::CONFIG_FILE_NAME))
}
