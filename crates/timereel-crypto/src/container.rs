//! The `.trc` capture container (architecture section 4.1, ADR 0004/0006).
//!
//! ```text
//! 'TRC1' | version u8 | thumb_len u32 | meta_len u32 | image_len u32
//!        | thumb age blob | meta age blob | image age blob
//! ```
//!
//! All integers are little-endian. Each part is its own age file encrypted to
//! the X25519 recipient, so each has a fresh file key. The framing is not
//! cryptography: it only lets a reader fetch the thumbnail (at the start of
//! the file) without reading the image.
//!
//! Each plaintext part is `part tag (1 byte) | capture id (16 bytes) | data`.
//! The reader checks the tag, and [`open_capture`] checks that all three ids
//! are equal, which detects parts swapped in from another capture.
//!
//! The parser never panics on any input and checks the magic, the version and
//! the length limits before it allocates.

use std::io::{self, Cursor, Read, Seek, SeekFrom, Write};
use std::str::FromStr;

use age::DecryptError;

/// First four bytes of every container.
pub const MAGIC: [u8; 4] = *b"TRC1";
/// Container format version.
pub const VERSION: u8 = 1;
/// Size of the fixed header: magic, version and three lengths.
pub const HEADER_LEN: usize = 4 + 1 + 3 * 4;
/// Size of the capture id in bytes.
pub const CAPTURE_ID_LEN: usize = 16;

/// Largest sealed (encrypted) thumbnail part, in bytes.
pub const MAX_THUMB_LEN: u32 = 1024 * 1024;
/// Largest sealed (encrypted) metadata part, in bytes.
pub const MAX_META_LEN: u32 = 64 * 1024;
/// Largest sealed (encrypted) image part, in bytes.
pub const MAX_IMAGE_LEN: u32 = 32 * 1024 * 1024;

/// One of the three parts of a capture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Part {
    /// The small thumbnail, stored first in the file.
    Thumb,
    /// The metadata JSON.
    Meta,
    /// The full image.
    Image,
}

impl Part {
    /// The tag that starts the plaintext of this part.
    fn tag(self) -> u8 {
        match self {
            Part::Thumb => 1,
            Part::Meta => 2,
            Part::Image => 3,
        }
    }

    fn max_len(self) -> u32 {
        match self {
            Part::Thumb => MAX_THUMB_LEN,
            Part::Meta => MAX_META_LEN,
            Part::Image => MAX_IMAGE_LEN,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Part::Thumb => "thumbnail",
            Part::Meta => "metadata",
            Part::Image => "image",
        }
    }
}

/// A random 16-byte id shared by the three parts of one capture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CaptureId(pub [u8; CAPTURE_ID_LEN]);

impl CaptureId {
    /// A new random id from the OS random generator.
    pub fn random() -> Result<Self, ContainerError> {
        let mut id = [0u8; CAPTURE_ID_LEN];
        getrandom::fill(&mut id).map_err(|e| ContainerError::Seal(e.to_string()))?;
        Ok(CaptureId(id))
    }
}

/// Errors of sealing and opening captures.
#[derive(Debug, thiserror::Error)]
pub enum ContainerError {
    /// The data ends before the part is complete.
    #[error("The capture file is incomplete")]
    Truncated,
    /// A part was changed, or parts do not belong together.
    #[error("The capture file was changed or damaged")]
    Tampered,
    /// The part is not encrypted to this key.
    #[error("The capture was not encrypted for this key")]
    WrongKey,
    /// Not a capture file: bad magic, version, length or part tag.
    #[error("Not a valid capture file: {0}")]
    BadFormat(&'static str),
    /// Sealing failed: bad public key, a part is too large or encryption failed.
    #[error("Cannot encrypt the capture: {0}")]
    Seal(String),
}

/// The part lengths read from a container header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    thumb_len: u32,
    meta_len: u32,
    image_len: u32,
}

impl Header {
    /// Parses and checks the fixed header: magic, version and the length
    /// limits. Nothing is allocated.
    pub fn parse(bytes: &[u8]) -> Result<Header, ContainerError> {
        if bytes.len() < HEADER_LEN {
            // Even a short prefix must start like a container.
            let n = bytes.len().min(MAGIC.len());
            if bytes[..n] != MAGIC[..n] {
                return Err(ContainerError::BadFormat("wrong magic"));
            }
            return Err(ContainerError::Truncated);
        }
        if bytes[..4] != MAGIC {
            return Err(ContainerError::BadFormat("wrong magic"));
        }
        if bytes[4] != VERSION {
            return Err(ContainerError::BadFormat("unsupported version"));
        }
        let len_at =
            |i: usize| u32::from_le_bytes([bytes[5 + i], bytes[6 + i], bytes[7 + i], bytes[8 + i]]);
        let header = Header {
            thumb_len: len_at(0),
            meta_len: len_at(4),
            image_len: len_at(8),
        };
        for part in [Part::Thumb, Part::Meta, Part::Image] {
            let len = header.len(part);
            if len == 0 {
                return Err(ContainerError::BadFormat("empty part"));
            }
            if len > part.max_len() {
                return Err(ContainerError::BadFormat("part too large"));
            }
        }
        Ok(header)
    }

    /// Length in bytes of the sealed part.
    pub fn len(&self, part: Part) -> u32 {
        match part {
            Part::Thumb => self.thumb_len,
            Part::Meta => self.meta_len,
            Part::Image => self.image_len,
        }
    }

    /// Offset of the sealed part from the start of the file.
    pub fn offset(&self, part: Part) -> u64 {
        let base = HEADER_LEN as u64;
        match part {
            Part::Thumb => base,
            Part::Meta => base + u64::from(self.thumb_len),
            Part::Image => base + u64::from(self.thumb_len) + u64::from(self.meta_len),
        }
    }

    /// Total file size the header promises.
    pub fn total_len(&self) -> u64 {
        self.offset(Part::Image) + u64::from(self.image_len)
    }
}

/// Encrypts one part to the recipient as an age file, in memory.
fn seal_part(
    recipient: &age::x25519::Recipient,
    part: Part,
    id: &CaptureId,
    data: &[u8],
) -> Result<Vec<u8>, ContainerError> {
    let seal_err = |e: &dyn std::fmt::Display| ContainerError::Seal(e.to_string());
    let enc = age::Encryptor::with_recipients(std::iter::once(recipient as &dyn age::Recipient))
        .map_err(|e| seal_err(&e))?;
    let mut out = Vec::with_capacity(data.len() + 512);
    let mut w = enc.wrap_output(&mut out).map_err(|e| seal_err(&e))?;
    w.write_all(&[part.tag()]).map_err(|e| seal_err(&e))?;
    w.write_all(&id.0).map_err(|e| seal_err(&e))?;
    w.write_all(data).map_err(|e| seal_err(&e))?;
    w.finish().map_err(|e| seal_err(&e))?;
    match u32::try_from(out.len()) {
        Ok(n) if n <= part.max_len() => Ok(out),
        _ => Err(ContainerError::Seal(format!(
            "the {} is too large (limit {} bytes)",
            part.name(),
            part.max_len()
        ))),
    }
}

/// Seals a capture into a `.trc` container with a new random capture id.
///
/// Needs only the public key (`age1...`). The three parts are encrypted in
/// memory, each as its own age file with a fresh file key.
pub fn seal_capture(
    public_key: &str,
    thumb: &[u8],
    meta: &[u8],
    image: &[u8],
) -> Result<Vec<u8>, ContainerError> {
    seal_capture_with_id(public_key, &CaptureId::random()?, thumb, meta, image)
}

/// Like [`seal_capture`] with a capture id chosen by the caller, so the same
/// id can also be written into the metadata JSON.
pub fn seal_capture_with_id(
    public_key: &str,
    id: &CaptureId,
    thumb: &[u8],
    meta: &[u8],
    image: &[u8],
) -> Result<Vec<u8>, ContainerError> {
    let recipient = age::x25519::Recipient::from_str(public_key)
        .map_err(|e| ContainerError::Seal(format!("invalid public key: {e}")))?;
    let thumb = seal_part(&recipient, Part::Thumb, id, thumb)?;
    let meta = seal_part(&recipient, Part::Meta, id, meta)?;
    let image = seal_part(&recipient, Part::Image, id, image)?;

    let mut out = Vec::with_capacity(HEADER_LEN + thumb.len() + meta.len() + image.len());
    out.extend_from_slice(&MAGIC);
    out.push(VERSION);
    for part in [&thumb, &meta, &image] {
        // Lengths were checked against the limits in `seal_part`.
        out.extend_from_slice(&(part.len() as u32).to_le_bytes());
    }
    out.extend_from_slice(&thumb);
    out.extend_from_slice(&meta);
    out.extend_from_slice(&image);
    Ok(out)
}

fn map_decrypt_error(e: DecryptError) -> ContainerError {
    match e {
        DecryptError::NoMatchingKeys => ContainerError::WrongKey,
        DecryptError::Io(e) => map_io_error(&e),
        DecryptError::InvalidHeader | DecryptError::UnknownFormat => {
            ContainerError::BadFormat("damaged age header")
        }
        _ => ContainerError::Tampered,
    }
}

fn map_io_error(e: &io::Error) -> ContainerError {
    match e.kind() {
        io::ErrorKind::UnexpectedEof => ContainerError::Truncated,
        _ => ContainerError::Tampered,
    }
}

/// Decrypts one sealed part and checks its tag. Returns the id and the data.
fn open_sealed(
    identity: &age::x25519::Identity,
    blob: &[u8],
    part: Part,
) -> Result<(CaptureId, Vec<u8>), ContainerError> {
    let dec = age::Decryptor::new(blob).map_err(map_decrypt_error)?;
    let mut r = dec
        .decrypt(std::iter::once(identity as &dyn age::Identity))
        .map_err(map_decrypt_error)?;
    // The blob is at most 32 MB (checked by the caller), so this is bounded.
    let mut plain = Vec::new();
    r.read_to_end(&mut plain).map_err(|e| map_io_error(&e))?;
    if plain.len() < 1 + CAPTURE_ID_LEN {
        return Err(ContainerError::BadFormat("part too short"));
    }
    if plain[0] != part.tag() {
        return Err(ContainerError::BadFormat("wrong part tag"));
    }
    let mut id = [0u8; CAPTURE_ID_LEN];
    id.copy_from_slice(&plain[1..1 + CAPTURE_ID_LEN]);
    plain.drain(..1 + CAPTURE_ID_LEN);
    Ok((CaptureId(id), plain))
}

/// Reads one sealed part from `reader`: the header, then only that part.
fn read_sealed<R: Read + Seek>(reader: &mut R, part: Part) -> Result<Vec<u8>, ContainerError> {
    reader
        .seek(SeekFrom::Start(0))
        .map_err(|e| map_io_error(&e))?;
    let mut head = Vec::with_capacity(HEADER_LEN);
    reader
        .by_ref()
        .take(HEADER_LEN as u64)
        .read_to_end(&mut head)
        .map_err(|e| map_io_error(&e))?;
    let header = Header::parse(&head)?;
    reader
        .seek(SeekFrom::Start(header.offset(part)))
        .map_err(|e| map_io_error(&e))?;
    // The length is within the limit; `take` + `read_to_end` still avoids
    // allocating the promised size up front for a short file.
    let want = u64::from(header.len(part));
    let mut blob = Vec::new();
    reader
        .by_ref()
        .take(want)
        .read_to_end(&mut blob)
        .map_err(|e| map_io_error(&e))?;
    if (blob.len() as u64) < want {
        return Err(ContainerError::Truncated);
    }
    Ok(blob)
}

/// Opens one part from a seekable reader (for example a file), reading only
/// the header and the bytes of that part. Returns the capture id and data.
///
/// The id lets the caller compare parts that it opened separately; use
/// [`open_capture`] to open and cross-check all three at once.
pub fn open_part_from<R: Read + Seek>(
    identity: &age::x25519::Identity,
    reader: &mut R,
    part: Part,
) -> Result<(CaptureId, Vec<u8>), ContainerError> {
    let blob = read_sealed(reader, part)?;
    open_sealed(identity, &blob, part)
}

/// Opens one part of a container held in memory and returns its data.
///
/// Only the header and the bytes of the requested part are read, so a
/// thumbnail can be opened from a buffer that holds just the start of the
/// file. Use [`open_capture`] to also check that all parts belong together.
pub fn open_part(
    identity: &age::x25519::Identity,
    bytes: &[u8],
    part: Part,
) -> Result<Vec<u8>, ContainerError> {
    open_part_from(identity, &mut Cursor::new(bytes), part).map(|(_, data)| data)
}

/// A fully opened capture.
#[derive(Debug, PartialEq, Eq)]
pub struct OpenedCapture {
    /// The id shared by all parts.
    pub id: CaptureId,
    /// Thumbnail bytes.
    pub thumb: Vec<u8>,
    /// Metadata bytes.
    pub meta: Vec<u8>,
    /// Image bytes.
    pub image: Vec<u8>,
}

/// Opens all three parts of a complete container and checks that the file
/// has exactly the promised size and that the capture id is the same in all
/// parts ([`ContainerError::Tampered`] otherwise).
pub fn open_capture(
    identity: &age::x25519::Identity,
    bytes: &[u8],
) -> Result<OpenedCapture, ContainerError> {
    let header = Header::parse(bytes)?;
    let total = header.total_len();
    if (bytes.len() as u64) < total {
        return Err(ContainerError::Truncated);
    }
    if (bytes.len() as u64) > total {
        return Err(ContainerError::BadFormat("extra bytes at the end"));
    }
    let mut cursor = Cursor::new(bytes);
    let (id_t, thumb) = open_part_from(identity, &mut cursor, Part::Thumb)?;
    let (id_m, meta) = open_part_from(identity, &mut cursor, Part::Meta)?;
    let (id_i, image) = open_part_from(identity, &mut cursor, Part::Image)?;
    if id_t != id_m || id_m != id_i {
        return Err(ContainerError::Tampered);
    }
    Ok(OpenedCapture {
        id: id_t,
        thumb,
        meta,
        image,
    })
}
