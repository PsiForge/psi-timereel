use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::Path;
use std::str::FromStr;

use argon2::{Algorithm, Argon2, Params, Version};
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use hkdf::Hkdf;
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use zeroize::Zeroizing;

use crate::error::KeyError;
use crate::recovery::RecoveryKey;

/// Version written to and required in `keyring.json`.
const VERSION: u32 = 1;
const SALT_LEN: usize = 16;
const NONCE_LEN: usize = 24;
const KEY_LEN: usize = 32;
const TAG_LEN: usize = 16;
const PASSWORD_SLOT: &str = "password";
const RECOVERY_SLOT: &str = "recovery";
const HKDF_INFO: &[u8] = b"timereel keyring v1 recovery slot";

// Upper limits for parameters read from a file, so that a damaged or
// hostile file cannot make us allocate unbounded memory or run for hours.
const MAX_M_KIB: u32 = 4 * 1024 * 1024;
const MAX_T: u32 = 64;
const MAX_P: u32 = 64;

/// Argon2id parameters. They are stored in the password slot, so they can
/// be raised later without breaking old files.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct KdfParams {
    /// Memory in KiB.
    pub m_kib: u32,
    /// Number of passes.
    pub t: u32,
    /// Degree of parallelism (lanes).
    pub p: u32,
}

impl KdfParams {
    /// Production parameters of ADR 0005: m = 256 MiB, t = 3, p = 4.
    pub const PRODUCTION: KdfParams = KdfParams {
        m_kib: 262_144,
        t: 3,
        p: 4,
    };
}

impl Default for KdfParams {
    fn default() -> Self {
        Self::PRODUCTION
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PasswordSlot {
    kdf: String,
    #[serde(flatten)]
    params: KdfParams,
    salt: String,
    nonce: String,
    ciphertext: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RecoverySlot {
    kdf: String,
    salt: String,
    nonce: String,
    ciphertext: String,
}

/// The key file: public key plus the encrypted private key in two slots.
/// It holds no plaintext secret, so it can be cloned and printed safely.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Keyring {
    version: u32,
    public_key: String,
    password_slot: PasswordSlot,
    recovery_slot: RecoverySlot,
}

impl Keyring {
    /// The `age1...` public key (given to the agent through `config.toml`).
    pub fn public_key(&self) -> &str {
        &self.public_key
    }

    /// The Argon2 parameters stored in the password slot.
    pub fn kdf_params(&self) -> KdfParams {
        self.password_slot.params
    }

    /// The Argon2 salt stored in the password slot.
    pub fn password_salt(&self) -> Result<Vec<u8>, KeyError> {
        decode_exact(&self.password_slot.salt, SALT_LEN, "salt")
    }

    /// Checks everything that can be checked without a secret.
    fn validate(&self) -> Result<(), KeyError> {
        if self.version != VERSION {
            return Err(corrupt(format!("unsupported version {}", self.version)));
        }
        age::x25519::Recipient::from_str(&self.public_key)
            .map_err(|_| corrupt("invalid public key".into()))?;
        let p = &self.password_slot;
        if p.kdf != "argon2id" {
            return Err(corrupt("unknown password slot kdf".into()));
        }
        let kp = p.params;
        if kp.m_kib == 0
            || kp.m_kib > MAX_M_KIB
            || kp.t == 0
            || kp.t > MAX_T
            || kp.p == 0
            || kp.p > MAX_P
            || kp.m_kib < 8 * kp.p
        {
            return Err(corrupt("Argon2 parameters out of range".into()));
        }
        decode_exact(&p.salt, SALT_LEN, "password salt")?;
        decode_exact(&p.nonce, NONCE_LEN, "password nonce")?;
        decode_min(&p.ciphertext, TAG_LEN, "password ciphertext")?;
        let r = &self.recovery_slot;
        if r.kdf != "hkdf-sha256" {
            return Err(corrupt("unknown recovery slot kdf".into()));
        }
        decode_exact(&r.salt, SALT_LEN, "recovery salt")?;
        decode_exact(&r.nonce, NONCE_LEN, "recovery nonce")?;
        decode_min(&r.ciphertext, TAG_LEN, "recovery ciphertext")?;
        Ok(())
    }
}

/// The opened private key. It is zeroized when dropped (the age identity
/// keeps its secret in a zeroizing wrapper).
pub struct UnlockedKey {
    identity: age::x25519::Identity,
}

impl std::fmt::Debug for UnlockedKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("UnlockedKey(<redacted>)")
    }
}

impl UnlockedKey {
    /// The age identity, for opening captures.
    pub fn identity(&self) -> &age::x25519::Identity {
        &self.identity
    }

    /// The matching `age1...` public key.
    pub fn public_key(&self) -> String {
        self.identity.to_public().to_string()
    }
}

fn corrupt(msg: String) -> KeyError {
    KeyError::Corrupt(msg)
}

fn decode_min(s: &str, min: usize, what: &str) -> Result<Vec<u8>, KeyError> {
    let v = B64
        .decode(s)
        .map_err(|_| corrupt(format!("{what} is not valid base64")))?;
    if v.len() < min {
        return Err(corrupt(format!("{what} is too short")));
    }
    Ok(v)
}

fn decode_exact(s: &str, len: usize, what: &str) -> Result<Vec<u8>, KeyError> {
    let v = decode_min(s, len, what)?;
    if v.len() != len {
        return Err(corrupt(format!("{what} has the wrong length")));
    }
    Ok(v)
}

fn random<const N: usize>() -> Result<[u8; N], KeyError> {
    let mut buf = [0u8; N];
    getrandom::fill(&mut buf).map_err(|e| KeyError::Crypto(format!("random bytes: {e}")))?;
    Ok(buf)
}

/// Associated data: binds the slot name and the public key to the ciphertext.
fn aad(slot: &str, public_key: &str) -> Vec<u8> {
    let mut v = Vec::new();
    v.extend_from_slice(b"timereel-keyring-v1\0");
    v.extend_from_slice(slot.as_bytes());
    v.push(0);
    v.extend_from_slice(public_key.as_bytes());
    v
}

#[cfg(test)]
thread_local! {
    /// Counting hook: how often Argon2 ran on this thread.
    pub(crate) static ARGON2_RUNS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

fn derive_password_key(
    password: &SecretString,
    salt: &[u8],
    params: KdfParams,
) -> Result<Zeroizing<[u8; KEY_LEN]>, KeyError> {
    #[cfg(test)]
    ARGON2_RUNS.with(|c| c.set(c.get() + 1));
    let p = Params::new(params.m_kib, params.t, params.p, Some(KEY_LEN))
        .map_err(|e| KeyError::Crypto(format!("Argon2 parameters: {e}")))?;
    let mut out = Zeroizing::new([0u8; KEY_LEN]);
    Argon2::new(Algorithm::Argon2id, Version::V0x13, p)
        .hash_password_into(password.expose_secret().as_bytes(), salt, &mut out[..])
        .map_err(|e| KeyError::Crypto(format!("Argon2: {e}")))?;
    Ok(out)
}

fn derive_recovery_key(
    recovery: &RecoveryKey,
    salt: &[u8],
) -> Result<Zeroizing<[u8; KEY_LEN]>, KeyError> {
    let mut out = Zeroizing::new([0u8; KEY_LEN]);
    Hkdf::<Sha256>::new(Some(salt), recovery.bytes())
        .expand(HKDF_INFO, &mut out[..])
        .map_err(|_| KeyError::Crypto("HKDF".into()))?;
    Ok(out)
}

fn seal(
    key: &[u8; KEY_LEN],
    nonce: &[u8; NONCE_LEN],
    aad: &[u8],
    plaintext: &[u8],
) -> Result<Vec<u8>, KeyError> {
    XChaCha20Poly1305::new(key.into())
        .encrypt(
            &XNonce::from(*nonce),
            Payload {
                msg: plaintext,
                aad,
            },
        )
        .map_err(|_| KeyError::Crypto("encryption failed".into()))
}

/// Opens a slot and checks that the identity matches the public key.
fn open_identity(
    key: &[u8; KEY_LEN],
    nonce: &[u8],
    ciphertext: &[u8],
    aad: &[u8],
    public_key: &str,
) -> Result<UnlockedKey, KeyError> {
    let nonce = XNonce::try_from(nonce).map_err(|_| KeyError::WrongSecret)?;
    let plain = XChaCha20Poly1305::new(key.into())
        .decrypt(
            &nonce,
            Payload {
                msg: ciphertext,
                aad,
            },
        )
        .map_err(|_| KeyError::WrongSecret)?;
    let plain = Zeroizing::new(plain);
    let text = std::str::from_utf8(&plain).map_err(|_| KeyError::WrongSecret)?;
    let identity = age::x25519::Identity::from_str(text).map_err(|_| KeyError::WrongSecret)?;
    if identity.to_public().to_string() != public_key {
        return Err(KeyError::WrongSecret);
    }
    Ok(UnlockedKey { identity })
}

/// Creates a new key pair and wraps it in a password slot and a recovery
/// slot. Runs Argon2 once. Returns the key file, the recovery key (to be
/// shown once and never stored) and the unlocked key.
pub fn create(
    password: &SecretString,
    params: KdfParams,
) -> Result<(Keyring, RecoveryKey, UnlockedKey), KeyError> {
    let identity = age::x25519::Identity::generate();
    let public_key = identity.to_public().to_string();
    let secret = identity.to_string();
    let secret_bytes = secret.expose_secret().as_bytes();

    let pw_salt = random::<SALT_LEN>()?;
    let pw_nonce = random::<NONCE_LEN>()?;
    let pw_key = derive_password_key(password, &pw_salt, params)?;
    let pw_ct = seal(
        &pw_key,
        &pw_nonce,
        &aad(PASSWORD_SLOT, &public_key),
        secret_bytes,
    )?;

    let recovery = RecoveryKey::from_bytes(Zeroizing::new(random::<32>()?));
    let rc_salt = random::<SALT_LEN>()?;
    let rc_nonce = random::<NONCE_LEN>()?;
    let rc_key = derive_recovery_key(&recovery, &rc_salt)?;
    let rc_ct = seal(
        &rc_key,
        &rc_nonce,
        &aad(RECOVERY_SLOT, &public_key),
        secret_bytes,
    )?;

    let keyring = Keyring {
        version: VERSION,
        public_key,
        password_slot: PasswordSlot {
            kdf: "argon2id".into(),
            params,
            salt: B64.encode(pw_salt),
            nonce: B64.encode(pw_nonce),
            ciphertext: B64.encode(pw_ct),
        },
        recovery_slot: RecoverySlot {
            kdf: "hkdf-sha256".into(),
            salt: B64.encode(rc_salt),
            nonce: B64.encode(rc_nonce),
            ciphertext: B64.encode(rc_ct),
        },
    };
    Ok((keyring, recovery, UnlockedKey { identity }))
}

/// Unlocks with the password. Argon2 runs exactly once. Any failure,
/// including a mismatching identity, is [`KeyError::WrongSecret`]; a
/// malformed key file is [`KeyError::Corrupt`].
pub fn unlock_with_password(
    keyring: &Keyring,
    password: &SecretString,
) -> Result<UnlockedKey, KeyError> {
    keyring.validate()?;
    let slot = &keyring.password_slot;
    let salt = decode_exact(&slot.salt, SALT_LEN, "password salt")?;
    let nonce = decode_exact(&slot.nonce, NONCE_LEN, "password nonce")?;
    let ct = decode_min(&slot.ciphertext, TAG_LEN, "password ciphertext")?;
    let key = derive_password_key(password, &salt, slot.params)?;
    open_identity(
        &key,
        &nonce,
        &ct,
        &aad(PASSWORD_SLOT, &keyring.public_key),
        &keyring.public_key,
    )
}

/// Unlocks with the recovery key as typed by the user (see
/// [`RecoveryKey::parse`]). A typo or wrong key is [`KeyError::WrongSecret`].
pub fn unlock_with_recovery_key(
    keyring: &Keyring,
    recovery_key: &SecretString,
) -> Result<UnlockedKey, KeyError> {
    keyring.validate()?;
    let recovery = RecoveryKey::parse(recovery_key)?;
    let slot = &keyring.recovery_slot;
    let salt = decode_exact(&slot.salt, SALT_LEN, "recovery salt")?;
    let nonce = decode_exact(&slot.nonce, NONCE_LEN, "recovery nonce")?;
    let ct = decode_min(&slot.ciphertext, TAG_LEN, "recovery ciphertext")?;
    let key = derive_recovery_key(&recovery, &salt)?;
    open_identity(
        &key,
        &nonce,
        &ct,
        &aad(RECOVERY_SLOT, &keyring.public_key),
        &keyring.public_key,
    )
}

/// Reads and checks a key file. A missing file is [`KeyError::Io`]; a
/// malformed one is [`KeyError::Corrupt`] (never a panic).
pub fn load(path: &Path) -> Result<Keyring, KeyError> {
    let bytes = fs::read(path)?;
    let keyring: Keyring = serde_json::from_slice(&bytes)
        .map_err(|e| corrupt(format!("cannot parse keyring.json: {e}")))?;
    keyring.validate()?;
    Ok(keyring)
}

/// Writes the key file atomically with mode 0600: a temp file in the same
/// directory, fsync, rename, fsync of the directory. A missing parent
/// directory is created with mode 0700.
pub fn save(keyring: &Keyring, path: &Path) -> Result<(), KeyError> {
    save_with_hook(keyring, path, &|| Ok(()))
}

/// Replaces the key file like [`save`]. `before_rename` runs after the temp
/// file is written and fsynced and before the rename; if it fails, the old
/// file is untouched (this is the failure-injection point for tests).
fn save_with_hook(
    keyring: &Keyring,
    path: &Path,
    before_rename: &dyn Fn() -> std::io::Result<()>,
) -> Result<(), KeyError> {
    let dir = key_dir(path);
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)?;
    let json = serde_json::to_vec_pretty(keyring)
        .map_err(|e| KeyError::Crypto(format!("cannot serialize key file: {e}")))?;
    let name = key_file_name(path)?;
    let tmp = dir.join(format!(".{name}.{}.tmp", std::process::id()));
    let result = (|| -> std::io::Result<()> {
        let mut f: File = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp)?;
        f.set_permissions(std::os::unix::fs::PermissionsExt::from_mode(0o600))?;
        f.write_all(&json)?;
        f.sync_all()?;
        before_rename()?;
        fs::rename(&tmp, path)?;
        File::open(dir)?.sync_all()
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    Ok(result?)
}

fn key_dir(path: &Path) -> &Path {
    match path.parent() {
        Some(d) if !d.as_os_str().is_empty() => d,
        _ => Path::new("."),
    }
}

fn key_file_name(path: &Path) -> Result<String, KeyError> {
    Ok(path
        .file_name()
        .ok_or_else(|| std::io::Error::other("key file path has no file name"))?
        .to_string_lossy()
        .into_owned())
}

/// Deletes temp files left behind by a crashed [`save`] or
/// [`change_password`] (`.<name>.<pid>.tmp` next to the key file). [`load`]
/// never reads them. Failing to delete one is not an error: it is only
/// clutter, and the next call tries again.
fn remove_leftover_temp_files(path: &Path) {
    let (Ok(name), Ok(entries)) = (key_file_name(path), fs::read_dir(key_dir(path))) else {
        return;
    };
    let prefix = format!(".{name}.");
    for entry in entries.flatten() {
        let file = entry.file_name();
        let file = file.to_string_lossy();
        if file.starts_with(&prefix)
            && file.ends_with(".tmp")
            && entry.file_type().is_ok_and(|t| t.is_file())
        {
            let _ = fs::remove_file(entry.path());
        }
    }
}

/// Changes the password by rewriting only the password slot.
///
/// `unlocked` is the key opened by the caller (with the current password or
/// the recovery key, see [`unlock_with_password`] and
/// [`unlock_with_recovery_key`]). The new slot gets a fresh random salt and
/// nonce and keeps the Argon2 parameters of the old slot. The public key and
/// the recovery slot are kept as they are, so captures and the recovery key
/// are not affected. The file is replaced atomically (see [`save`]); if
/// anything fails, the old file stays in place and still works. Leftover temp
/// files of earlier crashes are deleted. Argon2 runs once.
///
/// Returns the new key file. Fails with [`KeyError::Crypto`] if `unlocked`
/// does not belong to the key file at `keyring_path`.
pub fn change_password(
    keyring_path: &Path,
    unlocked: &UnlockedKey,
    new_password: &SecretString,
) -> Result<Keyring, KeyError> {
    change_password_with_hook(keyring_path, unlocked, new_password, &|| Ok(()))
}

fn change_password_with_hook(
    keyring_path: &Path,
    unlocked: &UnlockedKey,
    new_password: &SecretString,
    before_rename: &dyn Fn() -> std::io::Result<()>,
) -> Result<Keyring, KeyError> {
    let mut keyring = load(keyring_path)?;
    if unlocked.public_key() != keyring.public_key {
        return Err(KeyError::Crypto(
            "the unlocked key does not belong to this key file".into(),
        ));
    }
    let params = keyring.password_slot.params;
    let salt = random::<SALT_LEN>()?;
    let nonce = random::<NONCE_LEN>()?;
    let key = derive_password_key(new_password, &salt, params)?;
    let secret = unlocked.identity.to_string();
    let ct = seal(
        &key,
        &nonce,
        &aad(PASSWORD_SLOT, &keyring.public_key),
        secret.expose_secret().as_bytes(),
    )?;
    keyring.password_slot = PasswordSlot {
        kdf: "argon2id".into(),
        params,
        salt: B64.encode(salt),
        nonce: B64.encode(nonce),
        ciphertext: B64.encode(ct),
    };
    remove_leftover_temp_files(keyring_path);
    save_with_hook(&keyring, keyring_path, before_rename)?;
    remove_leftover_temp_files(keyring_path);
    Ok(keyring)
}

#[cfg(test)]
pub(crate) fn change_password_failing_before_rename(
    keyring_path: &Path,
    unlocked: &UnlockedKey,
    new_password: &SecretString,
) -> Result<Keyring, KeyError> {
    change_password_with_hook(keyring_path, unlocked, new_password, &|| {
        Err(std::io::Error::other("injected failure"))
    })
}
