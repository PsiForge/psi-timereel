//! The capture store: atomic writes, listing and single-part reads.

use std::fs::{self, File};
use std::io::Write;
use std::ops::{Bound, RangeBounds};
use std::path::{Path, PathBuf};

use age::x25519::Identity;
use serde::{Deserialize, Serialize};
use timereel_config::{create_private_dir_all, create_private_new, sync_dir};
use timereel_crypto::{open_part_from, seal_capture_with_id, CaptureId, Part};

use crate::error::StoreError;
use crate::time::{day_name, parse_day_name, CaptureTime, MS_PER_DAY};

/// Folder inside the data directory that holds the day folders.
pub const CAPTURES_DIR: &str = "captures";
/// File extension of a finished capture.
pub const CAPTURE_EXT: &str = "trc";
/// Extension of a capture that is being written.
pub const PARTIAL_EXT: &str = "partial";
/// Version of the metadata JSON.
pub const META_VERSION: u32 = 1;

/// The metadata of one capture. It is stored as JSON inside the encryption.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaptureMeta {
    /// When the capture was taken: Unix time in ms, UTC. It must equal the
    /// time in the file name.
    pub taken_at_ms: i64,
    /// Offset of the local time zone from UTC in minutes.
    pub tz_offset_min: i32,
    /// Name of the active app, or `unknown`.
    pub app_name: String,
    /// Title of the active window, or `unknown`.
    pub window_title: String,
    /// Width of the stored image in pixels.
    pub width: u32,
    /// Height of the stored image in pixels.
    pub height: u32,
    /// Image format of the stored image, for example `webp`.
    pub format: String,
}

/// The JSON as stored: the metadata plus the fields the store manages.
#[derive(Serialize, Deserialize)]
struct StoredMeta {
    version: u32,
    capture_id: String,
    #[serde(flatten)]
    meta: CaptureMeta,
}

fn hex(id: &CaptureId) -> String {
    id.0.iter().map(|b| format!("{b:02x}")).collect()
}

/// The encrypted capture store inside a data directory.
///
/// Layout (architecture section 4): `<data dir>/captures/YYYY-MM-DD/<UTC
/// time>.trc`. Writing needs only the public key; reading needs the identity.
#[derive(Debug, Clone)]
pub struct Store {
    root: PathBuf,
}

/// A capture that is fully written to its `.partial` file but not yet renamed.
pub(crate) struct Staged {
    pub(crate) partial: PathBuf,
    pub(crate) final_path: PathBuf,
    day_dir: PathBuf,
}

impl Store {
    /// A store in the data directory `data_dir`. Nothing is created on disk
    /// until the first write.
    pub fn new(data_dir: impl AsRef<Path>) -> Store {
        Store {
            root: data_dir.as_ref().join(CAPTURES_DIR),
        }
    }

    /// A store in the default data directory (`$TIMEREEL_DATA_DIR` or
    /// `$XDG_DATA_HOME/timereel`).
    pub fn at_default_location() -> Result<Store, timereel_config::PathError> {
        Ok(Store::new(timereel_config::data_dir()?))
    }

    /// The `captures` folder.
    pub fn captures_dir(&self) -> &Path {
        &self.root
    }

    fn day_dir(&self, time: CaptureTime) -> PathBuf {
        self.root.join(time.day_name())
    }

    /// Path of the finished capture file for `time`.
    pub fn path_of(&self, time: CaptureTime) -> PathBuf {
        self.day_dir(time)
            .join(format!("{}.{CAPTURE_EXT}", time.file_stem()))
    }

    /// Seals and stores one capture, atomically.
    ///
    /// The three parts are encrypted in memory to `public_key` (`age1...`).
    /// The file is written as `.<name>.partial`, fsynced, renamed to
    /// `<name>.trc`, and the folder is fsynced. A reader therefore sees
    /// either no file or a complete one. An existing capture at the same
    /// time is never overwritten ([`StoreError::AlreadyExists`]).
    pub fn write(
        &self,
        public_key: &str,
        meta: &CaptureMeta,
        thumb: &[u8],
        image: &[u8],
    ) -> Result<CaptureTime, StoreError> {
        let staged = self.stage(public_key, meta, thumb, image)?;
        self.commit(&staged)?;
        Ok(CaptureTime(meta.taken_at_ms))
    }

    /// Writes and fsyncs the `.partial` file, without renaming it.
    pub(crate) fn stage(
        &self,
        public_key: &str,
        meta: &CaptureMeta,
        thumb: &[u8],
        image: &[u8],
    ) -> Result<Staged, StoreError> {
        let time = CaptureTime(meta.taken_at_ms);
        if !time.is_representable() {
            return Err(StoreError::TimeOutOfRange(meta.taken_at_ms));
        }
        let id = CaptureId::random()?;
        let stored = StoredMeta {
            version: META_VERSION,
            capture_id: hex(&id),
            meta: meta.clone(),
        };
        let json = serde_json::to_vec(&stored)
            .map_err(|e| StoreError::BadMeta(format!("cannot serialise: {e}")))?;
        let bytes = seal_capture_with_id(public_key, &id, thumb, &json, image)?;

        let day_dir = self.day_dir(time);
        let stem = time.file_stem();
        let final_path = day_dir.join(format!("{stem}.{CAPTURE_EXT}"));
        let partial = day_dir.join(format!(".{stem}.{PARTIAL_EXT}"));

        let day_existed = day_dir.is_dir();
        create_private_dir_all(&self.root)
            .map_err(|e| StoreError::io("create folder", &self.root, e))?;
        create_private_dir_all(&day_dir)
            .map_err(|e| StoreError::io("create folder", &day_dir, e))?;
        if !day_existed {
            sync_dir(&self.root).map_err(|e| StoreError::io("sync folder", &self.root, e))?;
        }
        if final_path.exists() {
            return Err(StoreError::AlreadyExists(time));
        }

        let mut file =
            create_private_new(&partial).map_err(|e| StoreError::io("create", &partial, e))?;
        let written = file.write_all(&bytes).and_then(|()| file.sync_all());
        drop(file);
        if let Err(e) = written {
            // Best effort: the original error is the one that matters.
            let _ = fs::remove_file(&partial);
            return Err(StoreError::io("write", &partial, e));
        }
        Ok(Staged {
            partial,
            final_path,
            day_dir,
        })
    }

    /// Renames the staged file into place and fsyncs the folder.
    pub(crate) fn commit(&self, staged: &Staged) -> Result<(), StoreError> {
        if let Err(e) = fs::rename(&staged.partial, &staged.final_path) {
            let _ = fs::remove_file(&staged.partial);
            return Err(StoreError::io("rename", &staged.partial, e));
        }
        sync_dir(&staged.day_dir).map_err(|e| StoreError::io("sync folder", &staged.day_dir, e))
    }

    /// Lists the captures whose time is in `range`, oldest first.
    ///
    /// Works from folder and file names only: nothing is opened or
    /// decrypted. `.partial` files, symlinks and names that do not have the
    /// exact form of a capture file in its own day folder are ignored.
    pub fn list<R: RangeBounds<CaptureTime>>(
        &self,
        range: R,
    ) -> Result<Vec<CaptureTime>, StoreError> {
        let mut days: Vec<i64> = Vec::new();
        for entry in read_dir_or_empty(&self.root)? {
            let entry = entry.map_err(|e| StoreError::io("list", &self.root, e))?;
            let is_dir = entry.file_type().is_ok_and(|t| t.is_dir());
            let day = entry.file_name().to_str().and_then(parse_day_name);
            if let (true, Some(day)) = (is_dir, day) {
                if day_overlaps(&range, day) {
                    days.push(day);
                }
            }
        }
        days.sort_unstable();

        let mut out = Vec::new();
        for day in days {
            let dir = self.root.join(day_name(day));
            for entry in read_dir_or_empty(&dir)? {
                let entry = entry.map_err(|e| StoreError::io("list", &dir, e))?;
                if !entry.file_type().is_ok_and(|t| t.is_file()) {
                    continue;
                }
                let name = entry.file_name();
                let time = name
                    .to_str()
                    .and_then(|n| n.strip_suffix(&format!(".{CAPTURE_EXT}")))
                    .and_then(CaptureTime::parse_stem);
                if let Some(t) = time.filter(|t| t.day() == day && range.contains(t)) {
                    out.push(t);
                }
            }
        }
        out.sort_unstable();
        Ok(out)
    }

    /// Removes leftover `.partial` files from interrupted writes. Call it
    /// when the agent starts. Returns how many files were removed.
    ///
    /// Every file is tried; if some cannot be removed, the first error is
    /// returned after the others were handled.
    pub fn cleanup_partials(&self) -> Result<usize, StoreError> {
        let mut removed = 0;
        let mut first_err = None;
        for entry in read_dir_or_empty(&self.root)? {
            let entry = entry.map_err(|e| StoreError::io("list", &self.root, e))?;
            let is_day = entry.file_type().is_ok_and(|t| t.is_dir())
                && entry
                    .file_name()
                    .to_str()
                    .and_then(parse_day_name)
                    .is_some();
            if !is_day {
                continue;
            }
            let dir = entry.path();
            for f in read_dir_or_empty(&dir)? {
                let f = f.map_err(|e| StoreError::io("list", &dir, e))?;
                if !f.file_type().is_ok_and(|t| t.is_file()) || !is_partial_name(&f.file_name()) {
                    continue;
                }
                match fs::remove_file(f.path()) {
                    Ok(()) => removed += 1,
                    Err(e) => {
                        first_err.get_or_insert(StoreError::io("remove", &f.path(), e));
                    }
                }
            }
        }
        first_err.map_or(Ok(removed), Err)
    }

    /// Opens the capture file and checks its metadata against the file name.
    /// Returns the open file, the capture id and the verified metadata.
    fn open_verified(
        &self,
        identity: &Identity,
        time: CaptureTime,
    ) -> Result<(File, CaptureId, CaptureMeta), StoreError> {
        let path = self.path_of(time);
        let mut file = File::open(&path).map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => StoreError::NotFound(time),
            _ => StoreError::io("open", &path, e),
        })?;
        let (id, json) = open_part_from(identity, &mut file, Part::Meta)?;
        let stored: StoredMeta = serde_json::from_slice(&json)
            .map_err(|e| StoreError::BadMeta(format!("invalid JSON: {e}")))?;
        if stored.version != META_VERSION {
            return Err(StoreError::BadMeta(format!(
                "unsupported metadata version {}",
                stored.version
            )));
        }
        if stored.capture_id != hex(&id) {
            return Err(StoreError::BadMeta(
                "the capture id does not match the container".into(),
            ));
        }
        let inside = CaptureTime(stored.meta.taken_at_ms);
        if inside != time {
            return Err(StoreError::TimestampMismatch { file: time, inside });
        }
        Ok((file, id, stored.meta))
    }

    /// Reads and checks the metadata of the capture at `time`. Reads only the
    /// header and the metadata part of the file.
    pub fn read_meta(
        &self,
        identity: &Identity,
        time: CaptureTime,
    ) -> Result<CaptureMeta, StoreError> {
        self.open_verified(identity, time).map(|(_, _, meta)| meta)
    }

    /// Reads the thumbnail of the capture at `time`.
    ///
    /// Reads the header, the thumbnail and the small metadata part (to check
    /// that the file was not renamed and that the parts belong together). The
    /// image is not read.
    pub fn read_thumb(
        &self,
        identity: &Identity,
        time: CaptureTime,
    ) -> Result<Vec<u8>, StoreError> {
        self.read_checked_part(identity, time, Part::Thumb)
    }

    /// Reads the full image of the capture at `time`, after the same checks
    /// as [`Store::read_thumb`]. The thumbnail is not read.
    pub fn read_image(
        &self,
        identity: &Identity,
        time: CaptureTime,
    ) -> Result<Vec<u8>, StoreError> {
        self.read_checked_part(identity, time, Part::Image)
    }

    fn read_checked_part(
        &self,
        identity: &Identity,
        time: CaptureTime,
        part: Part,
    ) -> Result<Vec<u8>, StoreError> {
        let (mut file, meta_id, _) = self.open_verified(identity, time)?;
        let (id, data) = open_part_from(identity, &mut file, part)?;
        if id != meta_id {
            return Err(timereel_crypto::ContainerError::Tampered.into());
        }
        Ok(data)
    }
}

/// `.<capture stem>.partial`
fn is_partial_name(name: &std::ffi::OsStr) -> bool {
    name.to_str()
        .and_then(|n| n.strip_prefix('.'))
        .and_then(|n| n.strip_suffix(&format!(".{PARTIAL_EXT}")))
        .and_then(CaptureTime::parse_stem)
        .is_some()
}

/// Reads a directory; a missing directory is an empty one.
fn read_dir_or_empty(dir: &Path) -> Result<Vec<std::io::Result<fs::DirEntry>>, StoreError> {
    match fs::read_dir(dir) {
        Ok(rd) => Ok(rd.collect()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(StoreError::io("list", dir, e)),
    }
}

/// Whether the UTC day `day` can hold a time in `range`.
fn day_overlaps<R: RangeBounds<CaptureTime>>(range: &R, day: i64) -> bool {
    let (from, to) = (day * MS_PER_DAY, (day + 1) * MS_PER_DAY);
    let before_end = match range.end_bound() {
        Bound::Included(e) => e.0 >= from,
        Bound::Excluded(e) => e.0 > from,
        Bound::Unbounded => true,
    };
    let after_start = match range.start_bound() {
        Bound::Included(s) | Bound::Excluded(s) => s.0 < to,
        Bound::Unbounded => true,
    };
    before_end && after_start
}
