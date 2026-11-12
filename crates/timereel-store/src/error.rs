//! Errors of the capture store.

use std::io;
use std::path::PathBuf;

use timereel_crypto::ContainerError;

use crate::time::CaptureTime;

/// Everything that can go wrong in the store. Messages never contain image
/// data, app names or window titles.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// A file system operation failed.
    #[error("Could not {action} {}: {source}", path.display())]
    Io {
        /// What was being done, for example `write`.
        action: &'static str,
        /// The path involved.
        path: PathBuf,
        /// The system error.
        #[source]
        source: io::Error,
    },
    /// There is no capture at that time.
    #[error("There is no capture at {0}")]
    NotFound(CaptureTime),
    /// A capture already exists at that time. Captures are never overwritten.
    #[error("A capture at {0} already exists")]
    AlreadyExists(CaptureTime),
    /// The time cannot be written as a file name (year outside 0000-9999).
    #[error("The capture time {0} ms is outside the supported range")]
    TimeOutOfRange(i64),
    /// The metadata timestamp differs from the file name, so the file was
    /// renamed, moved or replaced.
    #[error(
        "The file name says the capture was taken at {file}, but the capture \
         says {inside}: the file was renamed or replaced"
    )]
    TimestampMismatch {
        /// Time from the file name.
        file: CaptureTime,
        /// Time inside the encrypted metadata.
        inside: CaptureTime,
    },
    /// The metadata is not valid, or does not belong to its container.
    #[error("The capture metadata is not valid: {0}")]
    BadMeta(String),
    /// Sealing or opening the container failed.
    #[error(transparent)]
    Container(#[from] ContainerError),
}

impl StoreError {
    pub(crate) fn io(action: &'static str, path: &std::path::Path, source: io::Error) -> Self {
        StoreError::Io {
            action,
            path: path.to_path_buf(),
            source,
        }
    }
}
