//! TimeReel `crypto` crate: the key file (`keyring.json`) of ADR 0005.
//!
//! One age X25519 identity is wrapped twice, like LUKS key slots:
//! a password slot (Argon2id + XChaCha20-Poly1305) and a recovery slot
//! (HKDF-SHA256 from a 256-bit recovery key + XChaCha20-Poly1305).
//!
//! Captures are sealed into the `.trc` container with age ([`seal_capture`],
//! [`open_part`], [`open_capture`]); see the `container` module for the format.
//!
//! Every unlock failure is [`KeyError::WrongSecret`]; a malformed file is
//! [`KeyError::Corrupt`]. Secrets live in `secrecy`/`zeroize` types.

mod container;
mod error;
mod keyring;
mod policy;
mod recovery;

#[cfg(test)]
mod container_tests;
#[cfg(test)]
mod tests;

pub use container::{
    open_capture, open_part, open_part_from, seal_capture, seal_capture_with_id, CaptureId,
    ContainerError, Header, OpenedCapture, Part, CAPTURE_ID_LEN, HEADER_LEN, MAGIC, MAX_IMAGE_LEN,
    MAX_META_LEN, MAX_THUMB_LEN, VERSION,
};
pub use error::KeyError;
pub use keyring::{
    create, load, save, unlock_with_password, unlock_with_recovery_key, KdfParams, Keyring,
    UnlockedKey,
};
pub use policy::{check_password, PasswordCheck, MIN_PASSWORD_CHARS};
pub use recovery::RecoveryKey;
pub use secrecy::{ExposeSecret, SecretString};
