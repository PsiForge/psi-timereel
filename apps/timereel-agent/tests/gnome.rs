//! The agent's Wayland active-window path against a private session bus
//! (`dbus-daemon --session`, the same thing `dbus-run-session` starts): with
//! the GNOME extension missing the app is unknown, and three skips in a row
//! raise the documented error state; with a fake extension the capture goes
//! through with the app and title.
#![allow(clippy::unwrap_used)]

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};

use age::x25519::Identity;
use timereel_agent::cycle::{AgentState, CycleRunner, Now, Outcome, SkipReason};
use timereel_capture::fake::{FakeLock, FakeScreen};
use timereel_capture::GnomeWindow;
use timereel_config::Config;
use timereel_store::{CaptureTime, Store};
use zbus::{connection, interface};

const T0: i64 = 1_747_215_000_000;

struct Bus {
    child: Child,
    address: String,
}

impl Bus {
    fn start() -> Bus {
        let mut child = Command::new("dbus-daemon")
            .args(["--session", "--nofork", "--print-address=1"])
            .stdout(Stdio::piped())
            .spawn()
            .expect("dbus-daemon is installed (package dbus)");
        let mut line = String::new();
        BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut line)
            .unwrap();
        Bus {
            child,
            address: line.trim().to_string(),
        }
    }
}

impl Drop for Bus {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

struct FakeFocus;

#[interface(name = "org.timereel.Focus1")]
impl FakeFocus {
    fn get_focus(&self) -> (bool, String, String, String) {
        (true, "Firefox".into(), "firefox".into(), "Inbox".into())
    }
}

fn now(n: i64) -> Now {
    Now {
        unix_ms: T0 + n * 1000,
        tz_offset_min: 0,
    }
}

struct Setup {
    dir: tempfile::TempDir,
    config: Config,
    key: Identity,
}

fn setup() -> Setup {
    let key = Identity::generate();
    Setup {
        dir: tempfile::tempdir().unwrap(),
        config: Config {
            public_key: Some(key.to_public().to_string()),
            ..Config::default()
        },
        key,
    }
}

fn runner(s: &Setup, window: GnomeWindow) -> CycleRunner {
    CycleRunner::new(
        Box::new(FakeScreen::single_monitor()),
        Box::new(window),
        Box::new(FakeLock::new(false)),
        Box::new(Store::new(s.dir.path())),
    )
    .warn_on_unknown_app(true)
}

#[test]
fn a_missing_extension_gives_three_skips_and_the_documented_error() {
    let bus = Bus::start();
    let s = setup();
    let mut r = runner(&s, GnomeWindow::with_address(bus.address.clone()));
    for n in 0..3 {
        assert_eq!(
            r.run_once(&s.config, now(n)),
            Outcome::Skipped(SkipReason::AppUnknown)
        );
    }
    assert_eq!(r.state(), AgentState::Error);
    assert_eq!(
        r.reason(),
        "Active app unknown: enable the TimeReel GNOME extension or allow capture when unknown"
    );
    assert_eq!(r.last_skip(), Some(SkipReason::AppUnknown));
    assert!(Store::new(s.dir.path()).list(..).unwrap().is_empty());
}

/// The fake extension, served from its own thread and runtime so the
/// synchronous runner can block on the bus. Stops when dropped.
struct Service {
    stop: Option<std::sync::mpsc::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Service {
    fn start(address: &str) -> Service {
        let address = address.to_string();
        let (stop, stopped) = std::sync::mpsc::channel::<()>();
        let (ready_tx, ready) = std::sync::mpsc::channel::<()>();
        let thread = std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            rt.block_on(async {
                let _conn = connection::Builder::address(address.as_str())
                    .name("org.timereel.Focus1")
                    .serve_at("/org/timereel/Focus1", FakeFocus)
                    .build()
                    .await
                    .unwrap();
                ready_tx.send(()).unwrap();
                // Keep serving until told to stop.
                while stopped.try_recv() == Err(std::sync::mpsc::TryRecvError::Empty) {
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                }
            });
        });
        ready.recv().unwrap();
        Service {
            stop: Some(stop),
            thread: Some(thread),
        }
    }
}

impl Drop for Service {
    fn drop(&mut self) {
        drop(self.stop.take());
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

#[test]
fn the_error_clears_once_the_extension_is_there_and_a_capture_succeeds() {
    let bus = Bus::start();
    let s = setup();
    let mut r = runner(&s, GnomeWindow::with_address(bus.address.clone()));
    for n in 0..3 {
        r.run_once(&s.config, now(n));
    }
    assert_eq!(r.state(), AgentState::Error);

    // The extension appears; the next cycle captures.
    let _service = Service::start(&bus.address);
    assert_eq!(
        r.run_once(&s.config, now(3)),
        Outcome::Captured(CaptureTime(T0 + 3000))
    );
    assert_eq!(r.state(), AgentState::Recording);
    assert_eq!(r.reason(), "");

    // The stored metadata has the app and title from the extension.
    let meta = Store::new(s.dir.path())
        .read_meta(&s.key, CaptureTime(T0 + 3000))
        .expect("capture can be opened");
    assert_eq!(meta.app_name, "Firefox");
    assert_eq!(meta.window_title, "Inbox");
}
