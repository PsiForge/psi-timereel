//! `Activate` from the tray while locked brings the lock screen to the
//! front. Uses a private session bus; this file has one test because it sets
//! `DBUS_SESSION_BUS_ADDRESS`.

mod common;

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use common::*;
use egui::{ViewportCommand, ViewportId};
use timereel::autolock::FakeClock;
use timereel::lock_ui::UNLOCK_LABEL;
use timereel::single::{activate_running, claim, Claim};

struct Bus(Child);

impl Drop for Bus {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn start_bus() -> Bus {
    let mut child = Command::new("dbus-daemon")
        .args(["--session", "--nofork", "--print-address=1"])
        .stdout(Stdio::piped())
        .spawn()
        .expect("dbus-daemon is installed (package dbus)");
    let mut line = String::new();
    BufReader::new(child.stdout.take().expect("stdout"))
        .read_line(&mut line)
        .expect("bus address");
    std::env::set_var("DBUS_SESSION_BUS_ADDRESS", line.trim());
    Bus(child)
}

#[test]
fn activate_while_locked_raises_the_lock_screen() {
    let _bus = start_bus();
    let Ok(Claim::Primary(primary)) = claim() else {
        panic!("the viewer must get the bus name");
    };
    let dir = tempfile::tempdir().expect("temp dir");
    let (path, _) = make_keyring(dir.path(), "correct horse battery");
    let mut h = viewer_harness(path, FakeClock::default());
    h.state_mut().attach_primary(primary);
    assert!(h.state().is_locked());

    activate_running().expect("activate");
    let start = Instant::now();
    loop {
        h.step();
        let raised = h
            .output()
            .viewport_output
            .get(&ViewportId::ROOT)
            .is_some_and(|v| {
                v.commands.contains(&ViewportCommand::Focus)
                    && v.commands.contains(&ViewportCommand::Visible(true))
            });
        if raised {
            break;
        }
        assert!(start.elapsed() < Duration::from_secs(5), "not raised");
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(h.state().is_locked());
    assert!(has_label(&h, UNLOCK_LABEL));
}
