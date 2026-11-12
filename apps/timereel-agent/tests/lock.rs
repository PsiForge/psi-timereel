//! Lock detection against fake logind and ScreenSaver services on a private
//! bus (`dbus-daemon --session`, the same thing `dbus-run-session` starts).
#![allow(clippy::unwrap_used)]

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use futures_util::StreamExt;
use timereel_agent::cycle::{CycleRunner, Now, Outcome, SkipReason};
use timereel_capture::fake::{FakeScreen, FakeWindow};
use timereel_capture::lock::DbusLockSource;
use timereel_capture::LockSource;
use timereel_config::Config;
use timereel_store::Store;
use zbus::object_server::SignalEmitter;
use zbus::{connection, interface, Connection};

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

    fn builder(&self) -> connection::Builder<'_> {
        connection::Builder::address(self.address.as_str())
    }

    async fn connect(&self) -> Connection {
        self.builder().build().await.unwrap()
    }
}

impl Drop for Bus {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

struct FakeSession {
    locked: AtomicBool,
}

#[interface(name = "org.freedesktop.login1.Session")]
impl FakeSession {
    #[zbus(property)]
    fn locked_hint(&self) -> bool {
        self.locked.load(Ordering::SeqCst)
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

async fn fake_logind(bus: &Bus, locked: bool) -> Connection {
    bus.builder()
        .name("org.freedesktop.login1")
        .serve_at(
            "/org/freedesktop/login1/session/auto",
            FakeSession {
                locked: AtomicBool::new(locked),
            },
        )
        .build()
        .await
        .unwrap()
}

async fn fake_gnome(bus: &Bus, active: bool) -> Connection {
    bus.builder()
        .name("org.gnome.ScreenSaver")
        .serve_at(
            "/org/gnome/ScreenSaver",
            FakeScreenSaver {
                active: AtomicBool::new(active),
            },
        )
        .build()
        .await
        .unwrap()
}

fn run_cycle(lock: DbusLockSource) -> Outcome {
    let dir = tempfile::tempdir().unwrap();
    let id = age::x25519::Identity::generate();
    let config = Config {
        public_key: Some(id.to_public().to_string()),
        ..Config::default()
    };
    let mut runner = CycleRunner::new(
        Box::new(FakeScreen::single_monitor()),
        Box::new(FakeWindow::known("Firefox", "Inbox")),
        Box::new(lock),
        Box::new(Store::new(dir.path())),
    );
    runner.run_once(
        &config,
        Now {
            unix_ms: 1_747_215_000_000,
            tz_offset_min: 0,
        },
    )
}

#[tokio::test(flavor = "current_thread")]
async fn logind_locked_hint_true_skips_with_reason_locked() {
    let bus = Bus::start();
    let _logind = fake_logind(&bus, true).await;
    let lock = DbusLockSource::start(Some(bus.connect().await), None).await;
    assert!(lock.is_locked());
    assert!(matches!(
        run_cycle(lock),
        Outcome::Skipped(SkipReason::Locked)
    ));
}

#[tokio::test(flavor = "current_thread")]
async fn logind_locked_hint_false_captures() {
    let bus = Bus::start();
    let _logind = fake_logind(&bus, false).await;
    let lock = DbusLockSource::start(Some(bus.connect().await), None).await;
    assert!(!lock.is_locked());
    assert!(matches!(run_cycle(lock), Outcome::Captured(_)));
}

#[tokio::test(flavor = "current_thread")]
async fn gnome_screensaver_is_used_when_logind_is_missing() {
    let bus = Bus::start();
    let _gnome = fake_gnome(&bus, true).await;
    // The same private bus plays both buses; it has no logind.
    let lock = DbusLockSource::start(Some(bus.connect().await), Some(bus.connect().await)).await;
    assert!(lock.is_locked());
    assert!(matches!(
        run_cycle(lock),
        Outcome::Skipped(SkipReason::Locked)
    ));
}

#[tokio::test(flavor = "current_thread")]
async fn lock_changed_stream_emits_on_a_fake_signal() {
    let bus = Bus::start();
    let gnome = fake_gnome(&bus, false).await;
    let lock = DbusLockSource::start(None, Some(bus.connect().await)).await;
    assert!(!lock.is_locked());
    let mut changes = Box::pin(lock.changes());

    let iface = gnome
        .object_server()
        .interface::<_, FakeScreenSaver>("/org/gnome/ScreenSaver")
        .await
        .unwrap();
    iface.get().await.active.store(true, Ordering::SeqCst);
    // The subscription may not be active yet on the bus: send until seen.
    let seen = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            FakeScreenSaver::active_changed(iface.signal_emitter(), true)
                .await
                .unwrap();
            if let Ok(Some(v)) =
                tokio::time::timeout(Duration::from_millis(100), changes.next()).await
            {
                return v;
            }
        }
    })
    .await
    .expect("stream emits");
    assert!(seen);
    assert!(lock.is_locked());
}
