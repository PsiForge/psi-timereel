//! File helpers that enforce the private modes of architecture section 4:
//! directories 0700, files 0600.

use std::fs::{self, DirBuilder, File, OpenOptions};
use std::io;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::Path;

/// Mode for every directory TimeReel creates.
pub const DIR_MODE: u32 = 0o700;
/// Mode for every file TimeReel creates.
pub const FILE_MODE: u32 = 0o600;

/// Creates `path` and any missing parents with mode 0700.
///
/// If `path` already exists with looser permissions they are tightened to
/// 0700, because TimeReel owns this directory.
pub fn create_private_dir_all(path: &Path) -> io::Result<()> {
    DirBuilder::new()
        .recursive(true)
        .mode(DIR_MODE)
        .create(path)?;
    let mode = fs::metadata(path)?.permissions().mode() & 0o777;
    if mode != DIR_MODE {
        fs::set_permissions(path, fs::Permissions::from_mode(DIR_MODE))?;
    }
    Ok(())
}

/// Opens (creating if needed) a file for read and write with mode 0600.
/// The content is not truncated.
pub fn open_private_rw(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(FILE_MODE)
        .open(path)
}

/// Creates a new file with mode 0600; fails if it already exists.
pub fn create_private_new(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(FILE_MODE)
        .open(path)
}

/// Flushes a directory's entries to disk (needed after a rename).
pub fn sync_dir(path: &Path) -> io::Result<()> {
    File::open(path)?.sync_all()
}
