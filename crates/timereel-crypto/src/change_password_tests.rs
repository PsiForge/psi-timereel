#![allow(clippy::unwrap_used)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use secrecy::SecretString;

use crate::keyring::change_password_failing_before_rename;
use crate::*;

/// Low parameters for fast tests. Never used in production.
const LOW: KdfParams = KdfParams {
    m_kib: 64,
    t: 1,
    p: 1,
};

fn pw(s: &str) -> SecretString {
    SecretString::from(s.to_string())
}

struct Fixture {
    _dir: tempfile::TempDir,
    path: PathBuf,
    recovery: SecretString,
    key: UnlockedKey,
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("keyring.json");
    let (ring, rk, key) = create(&pw("old password"), LOW).unwrap();
    save(&ring, &path).unwrap();
    Fixture {
        _dir: dir,
        path,
        recovery: rk.display(),
        key,
    }
}

fn raw(path: &Path) -> serde_json::Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

fn file_names(path: &Path) -> Vec<String> {
    let mut v: Vec<_> = fs::read_dir(path.parent().unwrap())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    v.sort();
    v
}

#[test]
fn new_password_unlocks_and_old_one_is_wrong_secret() {
    let f = fixture();
    change_password(&f.path, &f.key, &pw("new password")).unwrap();
    let ring = load(&f.path).unwrap();
    let key = unlock_with_password(&ring, &pw("new password")).unwrap();
    assert_eq!(key.public_key(), ring.public_key());
    assert!(matches!(
        unlock_with_password(&ring, &pw("old password")),
        Err(KeyError::WrongSecret)
    ));
}

#[test]
fn recovery_key_still_unlocks_after_the_change() {
    let f = fixture();
    change_password(&f.path, &f.key, &pw("new password")).unwrap();
    let ring = load(&f.path).unwrap();
    let key = unlock_with_recovery_key(&ring, &f.recovery).unwrap();
    assert_eq!(key.public_key(), f.key.public_key());
}

#[test]
fn change_works_after_unlocking_with_the_recovery_key() {
    let f = fixture();
    let ring = load(&f.path).unwrap();
    let key = unlock_with_recovery_key(&ring, &f.recovery).unwrap();
    change_password(&f.path, &key, &pw("after recovery")).unwrap();
    let ring = load(&f.path).unwrap();
    assert!(unlock_with_password(&ring, &pw("after recovery")).is_ok());
    assert!(unlock_with_recovery_key(&ring, &f.recovery).is_ok());
}

#[test]
fn capture_sealed_before_the_change_opens_after_it() {
    let f = fixture();
    let ring = load(&f.path).unwrap();
    let sealed = seal_capture(ring.public_key(), b"thumb", b"meta", b"image").unwrap();

    change_password(&f.path, &f.key, &pw("new password")).unwrap();

    let ring = load(&f.path).unwrap();
    let key = unlock_with_password(&ring, &pw("new password")).unwrap();
    let opened = open_capture(key.identity(), &sealed).unwrap();
    assert_eq!(opened.thumb, b"thumb");
    assert_eq!(opened.meta, b"meta");
    assert_eq!(opened.image, b"image");
}

#[test]
fn failure_before_the_rename_keeps_the_old_file_working() {
    let f = fixture();
    let before = fs::read(&f.path).unwrap();
    let err =
        change_password_failing_before_rename(&f.path, &f.key, &pw("new password")).unwrap_err();
    assert!(matches!(err, KeyError::Io(_)));

    // Byte-identical file, old password works, new one does not, no temp file.
    assert_eq!(fs::read(&f.path).unwrap(), before);
    let ring = load(&f.path).unwrap();
    assert!(unlock_with_password(&ring, &pw("old password")).is_ok());
    assert!(matches!(
        unlock_with_password(&ring, &pw("new password")),
        Err(KeyError::WrongSecret)
    ));
    assert_eq!(file_names(&f.path), ["keyring.json"]);

    // After the rename (a normal run), the new file works.
    change_password(&f.path, &f.key, &pw("new password")).unwrap();
    let ring = load(&f.path).unwrap();
    assert!(unlock_with_password(&ring, &pw("new password")).is_ok());
    assert!(matches!(
        unlock_with_password(&ring, &pw("old password")),
        Err(KeyError::WrongSecret)
    ));
}

#[test]
fn recovery_slot_and_public_key_are_byte_identical() {
    let f = fixture();
    let before = raw(&f.path);
    let before_ring = load(&f.path).unwrap();
    change_password(&f.path, &f.key, &pw("new password")).unwrap();
    let after = raw(&f.path);

    assert_eq!(before["recovery_slot"], after["recovery_slot"]);
    assert_eq!(before["public_key"], after["public_key"]);
    assert_eq!(before["version"], after["version"]);
    assert_eq!(
        load(&f.path).unwrap().public_key(),
        before_ring.public_key()
    );

    // The password slot is new: fresh salt, nonce and ciphertext, same params.
    let (b, a) = (&before["password_slot"], &after["password_slot"]);
    for field in ["salt", "nonce", "ciphertext"] {
        assert_ne!(b[field], a[field], "{field} must change");
    }
    for field in ["kdf", "m_kib", "t", "p"] {
        assert_eq!(b[field], a[field], "{field} must stay");
    }
}

#[test]
fn leftover_temp_files_are_ignored_on_load_and_cleaned_up() {
    let f = fixture();
    let dir = f.path.parent().unwrap();
    fs::write(dir.join(".keyring.json.99999.tmp"), b"half written garbage").unwrap();
    fs::write(dir.join("unrelated.tmp"), b"keep me").unwrap();

    // Load ignores the leftover.
    assert!(load(&f.path).is_ok());

    change_password(&f.path, &f.key, &pw("new password")).unwrap();
    assert_eq!(file_names(&f.path), ["keyring.json", "unrelated.tmp"]);
}

#[test]
fn file_keeps_mode_0600_and_returns_the_new_ring() {
    let f = fixture();
    let ring = change_password(&f.path, &f.key, &pw("new password")).unwrap();
    assert_eq!(
        fs::metadata(&f.path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert!(unlock_with_password(&ring, &pw("new password")).is_ok());
}

#[test]
fn a_key_of_another_key_file_is_refused_and_nothing_changes() {
    let f = fixture();
    let before = fs::read(&f.path).unwrap();
    let (_ring, _rk, other) = create(&pw("other"), LOW).unwrap();
    let err = change_password(&f.path, &other, &pw("new password")).unwrap_err();
    assert!(matches!(err, KeyError::Crypto(_)));
    assert_eq!(fs::read(&f.path).unwrap(), before);
}

#[test]
fn missing_or_corrupt_key_file_is_an_error() {
    let f = fixture();
    let missing = f.path.with_file_name("nope.json");
    assert!(matches!(
        change_password(&missing, &f.key, &pw("x")),
        Err(KeyError::Io(_))
    ));
    fs::write(&f.path, b"{not json").unwrap();
    assert!(matches!(
        change_password(&f.path, &f.key, &pw("x")),
        Err(KeyError::Corrupt(_))
    ));
}
