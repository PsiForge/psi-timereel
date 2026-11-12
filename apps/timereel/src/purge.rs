//! `timereel purge-data`: stop the capture service and delete every folder
//! TimeReel owns (data, config and state).
//!
//! The command lists the exact folders, asks for the word `DELETE`, runs
//! `systemctl --user disable --now timereel-agent.service` and only then
//! deletes. It removes nothing else: every folder is checked to be an absolute
//! path whose last part is `timereel` before anything happens, and a missing
//! folder is simply reported. `apt remove` never calls this (ADR 0012).

use std::ffi::OsString;
use std::io::{BufRead, Write};
use std::path::{Component, Path, PathBuf};

use timereel_config::{config_dir, data_dir, state_dir, PathError};

use crate::setup::{systemctl_user, AGENT_UNIT};

/// What the user has to type to confirm.
pub const CONFIRM_WORD: &str = "DELETE";
/// The last part every folder that may be deleted must have.
const REQUIRED_NAME: &str = "timereel";

/// What to delete and how to run `systemctl`. [`PurgeOptions::from_env`] is
/// what the real program uses; tests pass their own.
#[derive(Debug, Clone)]
pub struct PurgeOptions {
    /// The folders to delete (data, config, state), in this order.
    pub folders: Vec<PathBuf>,
    /// The program that is run as `systemctl --user disable --now <unit>`.
    pub systemctl: OsString,
}

impl PurgeOptions {
    /// The TimeReel data, config and state folders of the current user,
    /// honouring the `TIMEREEL_*_DIR` overrides, and the real `systemctl`.
    ///
    /// # Errors
    /// Returns [`PurgeError::Path`] if a folder cannot be determined.
    pub fn from_env() -> Result<Self, PurgeError> {
        Ok(Self {
            folders: vec![data_dir()?, config_dir()?, state_dir()?],
            systemctl: OsString::from("systemctl"),
        })
    }
}

/// How a purge ended when nothing went wrong.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// The user did not type `DELETE`; nothing was changed.
    Cancelled,
    /// The service was disabled and the folders were removed (or were missing).
    Purged,
}

/// Why a purge stopped. Every variant has a message for the user.
#[derive(Debug, thiserror::Error)]
pub enum PurgeError {
    /// A folder could not be determined.
    #[error("{0}")]
    Path(#[from] PathError),
    /// A folder is not a TimeReel folder. Nothing was changed.
    #[error("Refusing to delete {}: it is not an absolute path ending in \"{REQUIRED_NAME}\". Nothing was deleted.", .0.display())]
    NotTimeReelFolder(PathBuf),
    /// The prompt could not be read or written.
    #[error("Could not talk to the terminal: {0}")]
    Terminal(#[from] std::io::Error),
    /// `systemctl` failed; the folders were not touched.
    #[error("Could not stop and disable {AGENT_UNIT}: {0}\nNothing was deleted. Fix the problem above and run the command again.")]
    Service(String),
    /// Some folders could not be removed.
    #[error("Some folders could not be deleted:\n{}", .0.join("\n"))]
    Delete(Vec<String>),
}

/// Checks that `path` may be deleted: absolute, no `..` and the last part is
/// exactly `timereel`.
///
/// # Errors
/// Returns [`PurgeError::NotTimeReelFolder`] otherwise.
pub fn check_folder(path: &Path) -> Result<(), PurgeError> {
    let ok = path.is_absolute()
        && path.file_name().is_some_and(|n| n == REQUIRED_NAME)
        && !path.components().any(|c| c == Component::ParentDir);
    if ok {
        Ok(())
    } else {
        Err(PurgeError::NotTimeReelFolder(path.to_owned()))
    }
}

/// Runs the whole command: check, list, confirm, stop the service, delete.
///
/// Reads one line from `input`; anything but `DELETE` (including end of input)
/// cancels without changing anything. Messages go to `out`.
///
/// # Errors
/// See [`PurgeError`]. Nothing is deleted unless `DELETE` was typed and
/// `systemctl` succeeded.
pub fn run(
    options: &PurgeOptions,
    input: &mut impl BufRead,
    out: &mut impl Write,
) -> Result<Outcome, PurgeError> {
    // Check everything first, so a bad path stops the command before any prompt.
    let mut folders: Vec<&Path> = Vec::new();
    for folder in &options.folders {
        check_folder(folder)?;
        if !folders.contains(&folder.as_path()) {
            folders.push(folder);
        }
    }

    writeln!(out, "This will stop TimeReel and delete ALL of its data:")?;
    for folder in &folders {
        writeln!(out, "  {}", folder.display())?;
    }
    writeln!(
        out,
        "The screenshots, the key file (and with it the password and recovery key), the settings and the permission token are lost for good."
    )?;
    write!(out, "Type {CONFIRM_WORD} to continue: ")?;
    out.flush()?;

    let mut answer = String::new();
    input.read_line(&mut answer)?;
    if answer.trim_end_matches(['\r', '\n']) != CONFIRM_WORD {
        writeln!(out, "\nNothing was deleted.")?;
        return Ok(Outcome::Cancelled);
    }

    systemctl_user(&options.systemctl, &["disable", "--now", AGENT_UNIT])
        .map_err(PurgeError::Service)?;
    writeln!(out, "Stopped and disabled {AGENT_UNIT}.")?;

    let mut failures = Vec::new();
    for folder in folders {
        match remove_folder(folder) {
            Ok(true) => writeln!(out, "Deleted {}", folder.display())?,
            Ok(false) => writeln!(out, "Not found, nothing to delete: {}", folder.display())?,
            Err(e) => failures.push(format!("  {}: {e}", folder.display())),
        }
    }
    if failures.is_empty() {
        writeln!(
            out,
            "Done. You can now remove the programs with: sudo apt remove timereel"
        )?;
        Ok(Outcome::Purged)
    } else {
        Err(PurgeError::Delete(failures))
    }
}

/// Removes one checked folder. A symbolic link is removed itself, never
/// followed. Returns `false` if there was nothing to remove.
fn remove_folder(path: &Path) -> std::io::Result<bool> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.is_dir() => std::fs::remove_dir_all(path).map(|()| true),
        Ok(_) => std::fs::remove_file(path).map(|()| true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    struct Fixture {
        root: tempfile::TempDir,
        options: PurgeOptions,
    }

    impl Fixture {
        /// Three TimeReel folders with a file each, plus a sibling of each.
        fn new(systemctl_script: &str) -> Self {
            let root = tempfile::tempdir().expect("tempdir");
            let mut folders = Vec::new();
            for kind in ["data", "config", "state"] {
                let base = root.path().join(kind);
                fs::create_dir_all(base.join("timereel/sub")).expect("mkdir");
                fs::write(base.join("timereel/sub/file"), "x").expect("write");
                fs::create_dir_all(base.join("other")).expect("mkdir");
                fs::write(base.join("other/keep"), "x").expect("write");
                folders.push(base.join("timereel"));
            }
            let fake = root.path().join("fake-systemctl");
            fs::write(&fake, systemctl_script).expect("script");
            fs::set_permissions(&fake, fs::Permissions::from_mode(0o755)).expect("chmod");
            let options = PurgeOptions {
                folders,
                systemctl: fake.into_os_string(),
            };
            Self { root, options }
        }

        /// A fake `systemctl` that logs its arguments and succeeds.
        fn ok() -> Self {
            let f = Self::new("");
            let log = f.root.path().join("calls.log");
            let script = format!("#!/bin/sh\necho \"$@\" >> '{}'\n", log.display());
            fs::write(&f.options.systemctl, script).expect("script");
            f
        }

        fn calls(&self) -> String {
            fs::read_to_string(self.root.path().join("calls.log")).unwrap_or_default()
        }

        fn run(&self, typed: &str) -> (Result<Outcome, PurgeError>, String) {
            let mut out = Vec::new();
            let r = run(&self.options, &mut typed.as_bytes(), &mut out);
            (r, String::from_utf8(out).expect("utf8"))
        }

        fn exists(&self, rel: &str) -> bool {
            self.root.path().join(rel).exists()
        }
    }

    #[test]
    fn without_delete_nothing_is_deleted_or_run() {
        for typed in [
            "",
            "\n",
            "delete\n",
            "yes\n",
            "DELETE please\n",
            " DELETE\n",
        ] {
            let f = Fixture::ok();
            let (r, out) = f.run(typed);
            assert_eq!(r.expect("no error"), Outcome::Cancelled, "typed {typed:?}");
            assert!(out.contains("Nothing was deleted."));
            for kind in ["data", "config", "state"] {
                assert!(f.exists(&format!("{kind}/timereel/sub/file")), "{kind}");
            }
            assert_eq!(f.calls(), "", "systemctl must not run");
        }
    }

    #[test]
    fn the_folders_are_listed_before_asking() {
        let f = Fixture::ok();
        let (_, out) = f.run("no\n");
        for folder in &f.options.folders {
            assert!(out.contains(&folder.display().to_string()), "{out}");
        }
        assert!(out.contains("Type DELETE to continue: "));
    }

    #[test]
    fn with_delete_only_the_three_folders_go() {
        for typed in ["DELETE\n", "DELETE\r\n", "DELETE"] {
            let f = Fixture::ok();
            let (r, out) = f.run(typed);
            assert_eq!(r.expect("purged"), Outcome::Purged, "{out}");
            for kind in ["data", "config", "state"] {
                assert!(!f.exists(&format!("{kind}/timereel")), "{kind} still there");
                assert!(f.exists(&format!("{kind}/other/keep")), "{kind} sibling");
                assert!(f.exists(kind), "{kind} parent");
            }
            assert!(f.exists("fake-systemctl"));
        }
    }

    #[test]
    fn calls_systemctl_user_disable_now() {
        let f = Fixture::ok();
        f.run("DELETE\n").0.expect("purged");
        assert_eq!(f.calls(), "--user disable --now timereel-agent.service\n");
    }

    #[test]
    fn systemctl_failure_is_reported_and_nothing_is_deleted() {
        let f = Fixture::new(
            "#!/bin/sh\necho 'Failed to connect to bus: No medium found' >&2\nexit 1\n",
        );
        let (r, _) = f.run("DELETE\n");
        let msg = r.expect_err("must fail").to_string();
        assert!(msg.contains("timereel-agent.service"), "{msg}");
        assert!(msg.contains("exited with code 1"), "{msg}");
        assert!(msg.contains("Failed to connect to bus"), "{msg}");
        assert!(msg.contains("Nothing was deleted"), "{msg}");
        for kind in ["data", "config", "state"] {
            assert!(f.exists(&format!("{kind}/timereel/sub/file")), "{kind}");
        }
    }

    #[test]
    fn a_missing_systemctl_is_reported_and_nothing_is_deleted() {
        let mut f = Fixture::ok();
        f.options.systemctl = f.root.path().join("no-such-program").into_os_string();
        let (r, _) = f.run("DELETE\n");
        let msg = r.expect_err("must fail").to_string();
        assert!(msg.contains("Could not run systemctl"), "{msg}");
        assert!(f.exists("data/timereel/sub/file"));
    }

    #[test]
    fn a_path_not_ending_in_timereel_is_refused_before_anything_happens() {
        for bad in [
            "data/other",
            "data",
            "data/timereel-old",
            "data/Timereel",
            "data/timereel/..",
        ] {
            let mut f = Fixture::ok();
            let bad_path = f.root.path().join(bad);
            f.options.folders.push(bad_path);
            let (r, out) = f.run("DELETE\n");
            let msg = r.expect_err("must refuse").to_string();
            assert!(msg.contains("Refusing to delete"), "{bad}: {msg}");
            assert!(!out.contains("Type DELETE"), "no prompt for {bad}");
            assert_eq!(f.calls(), "", "{bad}: systemctl must not run");
            for kind in ["data", "config", "state"] {
                assert!(f.exists(&format!("{kind}/timereel/sub/file")), "{bad}");
                assert!(f.exists(&format!("{kind}/other/keep")), "{bad}");
            }
        }
    }

    #[test]
    fn relative_and_root_paths_are_refused() {
        assert!(check_folder(Path::new("timereel")).is_err());
        assert!(check_folder(Path::new("/")).is_err());
        assert!(check_folder(Path::new("")).is_err());
        assert!(check_folder(Path::new("/home/a/.local/share/timereel")).is_ok());
        assert!(check_folder(Path::new("/home/a/.local/share/timereel/")).is_ok());
    }

    #[test]
    fn missing_folders_are_not_an_error() {
        let f = Fixture::ok();
        fs::remove_dir_all(f.root.path().join("config/timereel")).expect("rm");
        let (r, out) = f.run("DELETE\n");
        assert_eq!(r.expect("purged"), Outcome::Purged);
        assert!(out.contains("Not found, nothing to delete"), "{out}");
        // Running it again with everything gone is still fine.
        let (r, _) = f.run("DELETE\n");
        assert_eq!(r.expect("purged again"), Outcome::Purged);
    }

    #[test]
    fn a_symlink_is_removed_but_not_followed() {
        let f = Fixture::ok();
        let target = f.root.path().join("data/other");
        let link = f.root.path().join("config/timereel");
        fs::remove_dir_all(&link).expect("rm");
        std::os::unix::fs::symlink(&target, &link).expect("symlink");
        f.run("DELETE\n").0.expect("purged");
        assert!(link.symlink_metadata().is_err(), "link removed");
        assert!(f.exists("data/other/keep"), "link target untouched");
    }

    #[test]
    fn the_same_folder_twice_is_listed_and_deleted_once() {
        let mut f = Fixture::ok();
        let again = f.options.folders[0].clone();
        f.options.folders.push(again);
        let (r, out) = f.run("DELETE\n");
        r.expect("purged");
        assert_eq!(out.matches("Deleted ").count(), 3, "{out}");
    }
}
