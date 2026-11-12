//! A second viewer calls `Activate` on the first one and exits. The test
//! starts a private session bus (`dbus-daemon --session`, what
//! `dbus-run-session` starts), so it also works outside `dbus-run-session`.

use std::io::{BufRead, BufReader};
use std::process::{Child, Command as Process, Stdio};
use std::time::Duration;

use timereel::single::{activate_running, claim, quit_running, Claim, Command};

/// A private bus daemon, killed when dropped.
struct Bus(Child);

impl Bus {
    fn start() -> Bus {
        let mut child = Process::new("dbus-daemon")
            .args(["--session", "--nofork", "--print-address=1"])
            .stdout(Stdio::piped())
            .spawn()
            .expect("dbus-daemon is installed (package dbus)");
        let mut line = String::new();
        let stdout = child.stdout.take().expect("piped stdout");
        BufReader::new(stdout)
            .read_line(&mut line)
            .expect("bus address");
        // This test file has one test, so changing the environment is safe.
        std::env::set_var("DBUS_SESSION_BUS_ADDRESS", line.trim());
        Bus(child)
    }
}

impl Drop for Bus {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn next(primary: &timereel::single::Primary) -> Option<Command> {
    for _ in 0..100 {
        if let Some(c) = primary.try_next() {
            return Some(c);
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    None
}

#[test]
fn a_second_instance_activates_the_first_and_does_not_start() {
    let _bus = Bus::start();
    let Ok(Claim::Primary(first)) = claim() else {
        panic!("the first viewer must get the name");
    };
    let second = claim();
    assert!(
        matches!(second, Ok(Claim::AlreadyRunning)),
        "the second viewer must see the name taken, got {:?}",
        second.as_ref().map(|_| "primary")
    );
    activate_running().expect("activate");
    assert_eq!(next(&first), Some(Command::Activate));
    quit_running().expect("quit");
    assert_eq!(next(&first), Some(Command::Quit));
    assert_eq!(first.try_next(), None);

    // Once the first viewer is gone the name is free again.
    drop(first);
    std::thread::sleep(Duration::from_millis(200));
    assert!(matches!(claim(), Ok(Claim::Primary(_))));
}
