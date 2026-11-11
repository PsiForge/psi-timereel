#![allow(clippy::unwrap_used)]

use std::os::unix::fs::PermissionsExt;

use secrecy::{ExposeSecret, SecretString};

use crate::keyring::ARGON2_RUNS;
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

fn runs() -> usize {
    ARGON2_RUNS.with(|c| c.get())
}

#[test]
fn right_password_unlocks_matching_identity() {
    let (ring, _rk, created) = create(&pw("correct horse battery"), LOW).unwrap();
    let key = unlock_with_password(&ring, &pw("correct horse battery")).unwrap();
    assert_eq!(key.public_key(), ring.public_key());
    assert_eq!(created.public_key(), ring.public_key());
    assert!(ring.public_key().starts_with("age1"));
}

#[test]
fn wrong_password_is_wrong_secret() {
    let (ring, _rk, _k) = create(&pw("correct horse battery"), LOW).unwrap();
    let err = unlock_with_password(&ring, &pw("wrong")).unwrap_err();
    assert!(matches!(err, KeyError::WrongSecret));
    assert_eq!(err.to_string(), "Password or recovery key is not correct");
}

#[test]
fn recovery_key_unlocks_and_typos_fail() {
    let (ring, rk, _k) = create(&pw("pw"), LOW).unwrap();
    let shown = rk.display();
    // Shown in groups, lowercase Bech32.
    assert!(shown.expose_secret().starts_with("trk1-"));
    let key = unlock_with_recovery_key(&ring, &shown).unwrap();
    assert_eq!(key.public_key(), ring.public_key());

    // Spacing and case are tolerated.
    let loose = pw(&shown.expose_secret().replace('-', " ").to_uppercase());
    assert!(unlock_with_recovery_key(&ring, &loose).is_ok());

    // A typo breaks the checksum.
    let mut typo: Vec<char> = shown.expose_secret().chars().collect();
    let i = typo.len() - 3;
    typo[i] = if typo[i] == 'q' { 'p' } else { 'q' };
    let typo = pw(&typo.into_iter().collect::<String>());
    assert!(matches!(
        unlock_with_recovery_key(&ring, &typo),
        Err(KeyError::WrongSecret)
    ));

    // A valid recovery key of another key file is wrong too.
    let (_ring2, rk2, _k2) = create(&pw("pw"), LOW).unwrap();
    assert!(matches!(
        unlock_with_recovery_key(&ring, &rk2.display()),
        Err(KeyError::WrongSecret)
    ));
    assert!(matches!(
        unlock_with_recovery_key(&ring, &pw("")),
        Err(KeyError::WrongSecret)
    ));
    assert!(matches!(
        unlock_with_recovery_key(&ring, &pw("not a key")),
        Err(KeyError::WrongSecret)
    ));
}

#[test]
fn file_has_no_secret_key_or_recovery_key() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("keyring.json");
    let (ring, rk, key) = create(&pw("a password"), LOW).unwrap();
    save(&ring, &path).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(!text.contains("AGE-SECRET-KEY"));
    assert!(!text.contains("a password"));
    let shown = rk.display();
    let compact: String = shown.expose_secret().replace('-', "");
    assert!(!text.contains(shown.expose_secret()));
    assert!(!text.contains(&compact));
    assert!(!text.to_lowercase().contains(&compact[compact.len() - 20..]));
    // Nothing else in the directory either.
    for entry in std::fs::read_dir(dir.path()).unwrap() {
        let content = std::fs::read(entry.unwrap().path()).unwrap();
        let s = String::from_utf8_lossy(&content);
        assert!(!s.contains("AGE-SECRET-KEY") && !s.contains(&compact));
    }
    // Debug output of secrets is redacted.
    assert!(!format!("{rk:?}").contains(&compact));
    assert!(!format!("{key:?}").contains("AGE-SECRET-KEY"));
}

#[test]
fn argon2_params_and_salt_are_stored_and_defaults_are_production() {
    assert_eq!(KdfParams::PRODUCTION.m_kib, 262_144);
    assert_eq!(KdfParams::PRODUCTION.t, 3);
    assert_eq!(KdfParams::PRODUCTION.p, 4);
    assert_eq!(KdfParams::default(), KdfParams::PRODUCTION);

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("keyring.json");
    let (ring, _rk, _k) = create(&pw("pw"), LOW).unwrap();
    save(&ring, &path).unwrap();
    let json: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    let slot = &json["password_slot"];
    assert_eq!(slot["m_kib"], 64);
    assert_eq!(slot["t"], 1);
    assert_eq!(slot["p"], 1);
    assert_eq!(slot["kdf"], "argon2id");
    assert!(slot["salt"].as_str().is_some_and(|s| !s.is_empty()));
    assert_eq!(json["version"], 1);

    let loaded = load(&path).unwrap();
    assert_eq!(loaded.kdf_params(), LOW);
    assert_eq!(loaded.password_salt().unwrap().len(), 16);
    assert_eq!(
        loaded.password_salt().unwrap(),
        ring.password_salt().unwrap()
    );

    // Two key files get different salts.
    let (ring2, _r, _k) = create(&pw("pw"), LOW).unwrap();
    assert_ne!(
        ring.password_salt().unwrap(),
        ring2.password_salt().unwrap()
    );
}

#[test]
fn argon2_runs_exactly_once_per_operation() {
    let before = runs();
    let (ring, rk, _k) = create(&pw("pw"), LOW).unwrap();
    assert_eq!(runs() - before, 1);

    let before = runs();
    unlock_with_password(&ring, &pw("pw")).unwrap();
    assert_eq!(runs() - before, 1);

    let before = runs();
    let _ = unlock_with_password(&ring, &pw("bad"));
    assert_eq!(runs() - before, 1);

    // The recovery key never needs Argon2.
    let before = runs();
    unlock_with_recovery_key(&ring, &rk.display()).unwrap();
    assert_eq!(runs() - before, 0);
}

#[test]
fn file_is_0600_and_save_replaces_atomically() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sub").join("keyring.json");
    let (ring, _rk, _k) = create(&pw("pw"), LOW).unwrap();
    save(&ring, &path).unwrap();
    let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
    let dmode = std::fs::metadata(path.parent().unwrap())
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(dmode, 0o700);
    // Saving again keeps the mode and leaves no temp files.
    save(&ring, &path).unwrap();
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let names: Vec<_> = std::fs::read_dir(path.parent().unwrap())
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(names.len(), 1);
}

#[test]
fn corrupt_files_give_corrupt_not_panic() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("keyring.json");
    let (ring, _rk, _k) = create(&pw("pw"), LOW).unwrap();
    save(&ring, &path).unwrap();
    let good = std::fs::read_to_string(&path).unwrap();

    let bad_inputs: Vec<String> = vec![
        String::new(),
        "not json".into(),
        "{}".into(),
        "null".into(),
        good[..good.len() / 2].to_string(),
        good.replace("\"version\": 1", "\"version\": 99"),
        good.replace("\"m_kib\": 64", "\"m_kib\": 4294967295"),
        good.replace("\"m_kib\": 64", "\"m_kib\": 0"),
        good.replace("\"t\": 1", "\"t\": 0"),
        good.replace("age1", "bge1"),
        good.replace("argon2id", "scrypt"),
        good.replace("\"salt\": \"", "\"salt\": \"!!"),
    ];
    for (i, input) in bad_inputs.iter().enumerate() {
        std::fs::write(&path, input).unwrap();
        let err = load(&path).unwrap_err();
        assert!(matches!(err, KeyError::Corrupt(_)), "case {i}: {err:?}");
    }
    std::fs::write(&path, [0xff, 0xfe, 0x00]).unwrap();
    assert!(matches!(load(&path), Err(KeyError::Corrupt(_))));

    // A missing file is an I/O error, not Corrupt.
    std::fs::remove_file(&path).unwrap();
    assert!(matches!(load(&path), Err(KeyError::Io(_))));
}

#[test]
fn unlock_rejects_a_ring_with_bad_lengths_without_panic() {
    let (ring, _rk, _k) = create(&pw("pw"), LOW).unwrap();
    let mut json: serde_json::Value = serde_json::to_value(&ring).unwrap();
    json["password_slot"]["nonce"] = "AAAA".into();
    let bad: Keyring = serde_json::from_value(json).unwrap();
    assert!(matches!(
        unlock_with_password(&bad, &pw("pw")),
        Err(KeyError::Corrupt(_))
    ));
}

#[test]
fn tampering_and_slot_swaps_are_wrong_secret() {
    let (ring, rk, _k) = create(&pw("pw"), LOW).unwrap();
    // Swap in another key file's public key: the associated data differs.
    let (other, _rk2, _k2) = create(&pw("pw"), LOW).unwrap();
    let mut json: serde_json::Value = serde_json::to_value(&ring).unwrap();
    json["public_key"] = other.public_key().into();
    let swapped: Keyring = serde_json::from_value(json).unwrap();
    assert!(matches!(
        unlock_with_password(&swapped, &pw("pw")),
        Err(KeyError::WrongSecret)
    ));
    assert!(matches!(
        unlock_with_recovery_key(&swapped, &rk.display()),
        Err(KeyError::WrongSecret)
    ));

    // A flipped ciphertext bit fails authentication.
    let mut json: serde_json::Value = serde_json::to_value(&ring).unwrap();
    let ct = json["password_slot"]["ciphertext"]
        .as_str()
        .unwrap()
        .to_string();
    let flipped = if ct.starts_with('A') { 'B' } else { 'A' };
    json["password_slot"]["ciphertext"] = format!("{flipped}{}", &ct[1..]).into();
    let tampered: Keyring = serde_json::from_value(json).unwrap();
    assert!(matches!(
        unlock_with_password(&tampered, &pw("pw")),
        Err(KeyError::WrongSecret)
    ));
}

#[test]
fn password_policy_warns_but_never_blocks() {
    assert_eq!(check_password(""), PasswordCheck::TooShort);
    assert_eq!(check_password("12345678901"), PasswordCheck::TooShort);
    assert_eq!(check_password("123456789012"), PasswordCheck::Ok);
    // Counted in characters, not bytes.
    assert_eq!(check_password("ääääääääääää"), PasswordCheck::Ok);
    assert!(PasswordCheck::TooShort.warning().is_some());
    assert!(PasswordCheck::Ok.warning().is_none());
    // A short password still creates a working key file.
    let (ring, _rk, _k) = create(&pw("short"), LOW).unwrap();
    assert!(unlock_with_password(&ring, &pw("short")).is_ok());
}
