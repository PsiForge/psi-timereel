//! TimeReel `store` crate: encrypted captures on disk (architecture section 4,
//! ADR 0006).
//!
//! # Where the data is
//!
//! The data directory is `$TIMEREEL_DATA_DIR` if set (tests), otherwise
//! `$XDG_DATA_HOME/timereel`, which is `~/.local/share/timereel`. Captures
//! live under it as
//!
//! ```text
//! <data dir>/captures/YYYY-MM-DD/<YYYYMMDDTHHMMSSmmmZ>.trc
//! <data dir>/captures/YYYY-MM-DD/.<YYYYMMDDTHHMMSSmmmZ>.partial   (while writing)
//! ```
//!
//! Day folders and file names use UTC. Folders are mode 0700 and files 0600.
//! Each `.trc` file is one container sealed with age (see `timereel-crypto`);
//! the capture time in the name is the only plaintext.
//!
//! # Guarantees
//!
//! * [`Store::write`] encrypts in memory, writes `.<name>.partial` in the same
//!   folder, fsyncs it, renames it and fsyncs the folder. Nothing is written
//!   outside the data directory and nothing unencrypted is written anywhere.
//! * [`Store::list`] uses file names only and ignores `.partial` files and
//!   unknown names. It needs no key.
//! * The readers check that the timestamp inside the metadata equals the file
//!   name and that all parts belong to the same capture.
//! * [`retention::run`] deletes captures older than the retention period, by
//!   file name only, and never follows symlinks.
//! * [`Store::cleanup_partials`] removes interrupted writes; the agent calls it
//!   at start.

mod error;
pub mod retention;
mod store;
mod time;

#[cfg(test)]
mod retention_tests;
#[cfg(test)]
mod tests;

pub use error::StoreError;
pub use store::{CaptureMeta, Store, CAPTURES_DIR, CAPTURE_EXT, META_VERSION, PARTIAL_EXT};
pub use time::CaptureTime;
