//! A fake desktop session-lock signal locks the viewer. The fake
//! `org.gnome.ScreenSaver` runs on a private bus (`dbus-daemon --session`).

mod common;

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use common::*;
use timereel::app::TIMELINE_PLACEHOLDER;
use timereel::autolock::FakeClock;
use timereel::lockwatch::LockWatch;
use zbus::object_server::SignalEmitter;
use zbus::{connection, interface, Connection};

struct Bus(Child, String);

impl Bus {
    fn start() -> Bus {
        let mut child = Command::new("dbus-daemon")
            .args(["--session", "--nofork", "--print-address=1"])
            .stdout(Stdio::piped())
            .spawn()
            .expect("dbus-daemon is installed (package dbus)");
        let mut line = String::new();
        BufReader::new(child.stdout.take().expect("stdout"))
            .read_line(&mut line)
            .expect("bus address");
        Bus(child, line.trim().to_owned())
    }
}

impl Drop for Bus {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

struct FakeScreenSaver {
    active: AtomicBool,
}

#[interface(name = "org.gnome.ScreenSaver")]
impl FakeScreenSaver {
    fn get_active(&self) -> bool {
        self.active.load(Ordering::SeqCst)
    }

    #[zbus(signal)]
    async fn active_changed(emitter: &SignalEmitter<'_>, active: bool) -> zbus::Result<()>;
}

#[test]
fn a_session_lock_signal_locks_the_viewer() {
    let bus = Bus::start();
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .expect("runtime");
    let address = bus.1.clone();
    let gnome: Connection = rt.block_on(async {
        connection::Builder::address(address.as_str())
            .name("org.gnome.ScreenSaver")
            .serve_at(
                "/org/gnome/ScreenSaver",
                FakeScreenSaver {
                    active: AtomicBool::new(false),
                },
            )
            .build()
            .await
            .expect("fake service")
    });
    let address = bus.1.clone();
    let watch = LockWatch::start(move || async move {
        let session = connection::Builder::address(address.as_str())
            .build()
            .await
            .ok();
        (None, session)
    })
    .expect("lock watch");

    let dir = tempfile::tempdir().expect("temp dir");
    let (path, _) = make_keyring(dir.path(), "correct horse battery");
    let mut h = viewer_harness(path, FakeClock::default());
    h.state_mut().attach_lock_watch(watch);
    unlock(&mut h, "correct horse battery", false);
    wait_until(&mut h, "timeline", |h| has_label(h, TIMELINE_PLACEHOLDER));

    // The watch may not be subscribed yet: send until the viewer locks.
    let start = Instant::now();
    while !h.state().is_locked() {
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "viewer did not lock"
        );
        rt.block_on(async {
            let iface = gnome
                .object_server()
                .interface::<_, FakeScreenSaver>("/org/gnome/ScreenSaver")
                .await
                .expect("iface");
            iface.get().await.active.store(true, Ordering::SeqCst);
            FakeScreenSaver::active_changed(iface.signal_emitter(), true)
                .await
                .expect("signal");
        });
        std::thread::sleep(Duration::from_millis(100));
        h.run_steps(1);
    }
    assert!(!h.state().is_unlocked());
}
