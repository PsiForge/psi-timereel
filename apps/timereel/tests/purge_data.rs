//! `timereel purge-data` run as the real program, with the test overrides
//! (`TIMEREEL_*_DIR`) and a fake `systemctl` first in `PATH`.

use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Output, Stdio};

fn make_tree(root: &Path) {
    for kind in ["data", "config", "state"] {
        let dir = root.join(kind);
        fs::create_dir_all(dir.join("timereel/captures")).expect("mkdir");
        fs::write(dir.join("timereel/captures/a.trc"), "x").expect("write");
        fs::create_dir_all(dir.join("sibling")).expect("mkdir");
        fs::write(dir.join("sibling/keep"), "x").expect("write");
    }
}

fn run(root: &Path, dirs: [&str; 3], script: &str, typed: &str) -> Output {
    let bin = root.join("bin");
    fs::create_dir_all(&bin).expect("mkdir");
    let fake = bin.join("systemctl");
    fs::write(&fake, script).expect("script");
    fs::set_permissions(&fake, fs::Permissions::from_mode(0o755)).expect("chmod");
    let mut child = Command::new(env!("CARGO_BIN_EXE_timereel"))
        .arg("purge-data")
        .env("PATH", &bin)
        .env("TIMEREEL_DATA_DIR", root.join(dirs[0]))
        .env("TIMEREEL_CONFIG_DIR", root.join(dirs[1]))
        .env("TIMEREEL_STATE_DIR", root.join(dirs[2]))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(typed.as_bytes())
        .expect("type");
    child.wait_with_output().expect("wait")
}

const DIRS: [&str; 3] = ["data/timereel", "config/timereel", "state/timereel"];

#[test]
fn delete_removes_only_the_three_folders() {
    let root = tempfile::tempdir().expect("tempdir");
    make_tree(root.path());
    let log = root.path().join("calls.log");
    let script = format!("#!/bin/sh\necho \"$@\" > '{}'\n", log.display());
    let out = run(root.path(), DIRS, &script, "DELETE\n");
    assert!(out.status.success(), "{out:?}");
    for kind in ["data", "config", "state"] {
        assert!(!root.path().join(kind).join("timereel").exists());
        assert!(root.path().join(kind).join("sibling/keep").exists());
    }
    assert_eq!(
        fs::read_to_string(log).expect("log"),
        "--user disable --now timereel-agent.service\n"
    );
}

#[test]
fn anything_else_deletes_nothing() {
    let root = tempfile::tempdir().expect("tempdir");
    make_tree(root.path());
    let out = run(root.path(), DIRS, "#!/bin/sh\nexit 0\n", "no\n");
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stdout).contains("Nothing was deleted."));
    for kind in ["data", "config", "state"] {
        assert!(root
            .path()
            .join(kind)
            .join("timereel/captures/a.trc")
            .exists());
    }
}

#[test]
fn an_override_not_ending_in_timereel_is_refused() {
    let root = tempfile::tempdir().expect("tempdir");
    make_tree(root.path());
    let out = run(
        root.path(),
        ["data/timereel", "config/timereel", "state/sibling"],
        "#!/bin/sh\nexit 0\n",
        "DELETE\n",
    );
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("Refusing to delete"));
    for kind in ["data", "config", "state"] {
        assert!(root.path().join(kind).join("sibling/keep").exists());
    }
    assert!(root.path().join("data/timereel/captures/a.trc").exists());
}

#[test]
fn systemctl_errors_are_shown_and_nothing_is_deleted() {
    let root = tempfile::tempdir().expect("tempdir");
    make_tree(root.path());
    let out = run(
        root.path(),
        DIRS,
        "#!/bin/sh\necho 'no user session' >&2\nexit 5\n",
        "DELETE\n",
    );
    assert_eq!(out.status.code(), Some(1));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("exited with code 5: no user session"), "{err}");
    assert!(root.path().join("data/timereel/captures/a.trc").exists());
}

#[test]
fn missing_folders_do_nothing_harmful() {
    let root = tempfile::tempdir().expect("tempdir");
    let out = run(root.path(), DIRS, "#!/bin/sh\nexit 0\n", "DELETE\n");
    assert!(out.status.success(), "{out:?}");
    assert!(String::from_utf8_lossy(&out.stdout).contains("Not found"));
}
