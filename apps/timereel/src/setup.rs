//! First-run set-up without any GUI: create the key, store it, tell the
//! config about the public key and start the capture service.
//!
//! Nothing is written until [`install`]. The recovery key only ever lives in
//! the [`NewKey`] in memory; no function here writes it anywhere.

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use timereel_config::{data_dir, update, ConfigError, PathError};
use timereel_crypto::{
    create, load, save, KdfParams, KeyError, Keyring, RecoveryKey, SecretString,
};

/// Name of the key file inside the data directory.
pub const KEYRING_FILE_NAME: &str = "keyring.json";
/// The systemd user unit that runs the agent.
pub const AGENT_UNIT: &str = "timereel-agent.service";

/// Knobs that tests change. [`Default`] is what the real program uses.
#[derive(Debug, Clone)]
pub struct SetupOptions {
    /// Argon2 parameters for the password slot.
    pub kdf: KdfParams,
    /// The program that is run as `systemctl --user enable --now <unit>`.
    pub systemctl: OsString,
}

impl Default for SetupOptions {
    fn default() -> Self {
        Self {
            kdf: KdfParams::PRODUCTION,
            systemctl: OsString::from("systemctl"),
        }
    }
}

/// Why set-up could not finish. Every variant has a message for the user.
#[derive(Debug, thiserror::Error)]
pub enum SetupError {
    /// A directory could not be determined.
    #[error("{0}")]
    Path(#[from] PathError),
    /// The key could not be created or saved.
    #[error("The key could not be created or saved: {0}")]
    Key(#[from] KeyError),
    /// The config could not be updated.
    #[error("The settings could not be saved: {0}")]
    Config(#[from] ConfigError),
    /// A key file with another key already exists; it is never overwritten.
    #[error("A key file already exists. TimeReel will not replace it, because that would make the saved screenshots unreadable.")]
    AlreadySetUp,
}

/// A fresh key pair that has not been stored yet.
pub struct NewKey {
    keyring: Keyring,
    recovery: RecoveryKey,
}

impl NewKey {
    /// The recovery key text to show once, in groups.
    pub fn recovery_text(&self) -> SecretString {
        self.recovery.display()
    }

    /// The key file contents that [`install`] will save.
    pub fn keyring(&self) -> &Keyring {
        &self.keyring
    }
}

/// Path of the key file.
///
/// # Errors
/// Fails if the data directory cannot be determined.
pub fn keyring_path() -> Result<PathBuf, SetupError> {
    Ok(data_dir()?.join(KEYRING_FILE_NAME))
}

/// True if a key file exists, which means set-up has been done.
///
/// # Errors
/// Fails if the data directory cannot be determined.
pub fn is_set_up() -> Result<bool, SetupError> {
    Ok(keyring_path()?.exists())
}

/// Creates the key pair. It runs Argon2, which takes a few seconds, so call
/// it on a worker thread. Nothing is written to disk.
///
/// # Errors
/// Fails if the key cannot be created.
pub fn create_key(password: &SecretString, options: &SetupOptions) -> Result<NewKey, SetupError> {
    let (keyring, recovery, _unlocked) = create(password, options.kdf)?;
    Ok(NewKey { keyring, recovery })
}

/// Saves the key file (mode 0600, atomic) and writes `public_key` to the
/// config. The key file goes first, so the agent never gets a public key
/// whose private half does not exist. Calling it again after a failed config
/// update is fine; an existing key file with a different key is never
/// replaced.
///
/// # Errors
/// Fails if either file cannot be written.
pub fn install(keyring: &Keyring) -> Result<(), SetupError> {
    let path = keyring_path()?;
    if path.exists() {
        let same = load(&path).is_ok_and(|k| k.public_key() == keyring.public_key());
        if !same {
            return Err(SetupError::AlreadySetUp);
        }
    } else {
        save(keyring, &path)?;
    }
    let public_key = keyring.public_key().to_owned();
    update(|c| c.public_key = Some(public_key))?;
    Ok(())
}

/// Runs `systemctl --user enable --now timereel-agent.service`.
///
/// # Errors
/// Returns a message for the user if the command cannot be started or fails.
pub fn enable_agent(options: &SetupOptions) -> Result<(), String> {
    let out = Command::new(&options.systemctl)
        .args(["--user", "enable", "--now", AGENT_UNIT])
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("Could not run systemctl: {e}."))?;
    if out.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    let detail: String = stderr.trim().chars().take(300).collect();
    let status = out.status.code().map_or_else(
        || "was stopped by a signal".to_owned(),
        |c| format!("exited with code {c}"),
    );
    Err(if detail.is_empty() {
        format!("systemctl {status}.")
    } else {
        format!("systemctl {status}: {detail}")
    })
}
