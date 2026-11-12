//! The portal restore token file (architecture section 4:
//! `$XDG_STATE_HOME/timereel/portal-restore-token`, mode 0600).
//!
//! The token is not encrypted (architecture section 4.2). It is single-use:
//! the portal gives a new one at every start, and [`TokenStore::save`]
//! replaces the old one atomically.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// File name inside the state directory.
pub const TOKEN_FILE_NAME: &str = "portal-restore-token";

/// A token longer than this is not a token; it is ignored.
const MAX_TOKEN_LEN: u64 = 4096;

/// Reads and writes the restore token file.
#[derive(Debug, Clone)]
pub struct TokenStore {
    path: PathBuf,
}

impl TokenStore {
    /// A store for the file `path`.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// The store at the default place in the state directory
    /// (`$TIMEREEL_STATE_DIR`, else `$XDG_STATE_HOME/timereel`).
    ///
    /// # Errors
    /// Fails if no state directory can be found.
    pub fn at_default_location() -> Result<Self, timereel_config::PathError> {
        Ok(Self::new(
            timereel_config::state_dir()?.join(TOKEN_FILE_NAME),
        ))
    }

    /// The path of the token file.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The stored token, or `None` if there is none (no file, empty file or an
    /// unreadable one).
    pub fn load(&self) -> Option<String> {
        let meta = fs::metadata(&self.path).ok()?;
        if !meta.is_file() || meta.len() > MAX_TOKEN_LEN {
            return None;
        }
        let text = fs::read_to_string(&self.path).ok()?;
        let token = text.trim();
        (!token.is_empty()).then(|| token.to_string())
    }

    /// Replaces the token: writes a temp file next to it with mode 0600,
    /// syncs it, renames it over the old one and syncs the directory.
    ///
    /// # Errors
    /// Any I/O error. The old token stays in place if the write fails.
    pub fn save(&self, token: &str) -> io::Result<()> {
        let dir = self
            .path
            .parent()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no parent directory"))?;
        timereel_config::create_private_dir_all(dir)?;
        let tmp = dir.join(format!(".{TOKEN_FILE_NAME}.partial"));
        // A leftover from a crash would make `create_private_new` fail.
        match fs::remove_file(&tmp) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        let result = (|| {
            let mut f = timereel_config::create_private_new(&tmp)?;
            f.write_all(token.as_bytes())?;
            f.sync_all()?;
            fs::rename(&tmp, &self.path)?;
            timereel_config::sync_dir(dir)
        })();
        if result.is_err() {
            // Best effort; the error that matters is the one above.
            let _ = fs::remove_file(&tmp);
        }
        result
    }
}
