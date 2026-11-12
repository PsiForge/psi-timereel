//! The GNOME extension client against a fake `org.timereel.Focus1` service on
//! a private session bus (`dbus-daemon --session`, the same thing
//! `dbus-run-session` starts).
#![allow(clippy::unwrap_used)]

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};

use timereel_capture::{ActiveWindow, ActiveWindowSource, GnomeWindow};
use zbus::{connection, interface, Connection};

/// A private bus daemon, killed when dropped.
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

/// The fake extension: answers `GetFocus` with a fixed reply.
struct FakeFocus(bool, &'static str, &'static str, &'static str);

#[interface(name = "org.timereel.Focus1")]
impl FakeFocus {
    fn get_focus(&self) -> (bool, String, String, String) {
        (
            self.0,
            self.1.to_string(),
            self.2.to_string(),
            self.3.to_string(),
        )
    }
}

async fn serve(bus: &Bus, reply: FakeFocus) -> Connection {
    connection::Builder::address(bus.address.as_str())
        .name("org.timereel.Focus1")
        .serve_at("/org/timereel/Focus1", reply)
        .build()
        .await
        .unwrap()
}

/// Calls the (synchronous) source from a plain thread, as the agent's
/// capture cycle does, without blocking the test's runtime.
async fn ask(source: GnomeWindow) -> ActiveWindow {
    tokio::task::spawn_blocking(move || source.active_window())
        .await
        .unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn returns_the_app_and_title() {
    let bus = Bus::start();
    let _service = serve(&bus, FakeFocus(true, "Firefox", "firefox", "Inbox - Mail")).await;
    assert_eq!(
        ask(GnomeWindow::with_address(bus.address.clone())).await,
        ActiveWindow::Known {
            app: "Firefox".into(),
            title: "Inbox - Mail".into()
        }
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn works_when_called_inside_a_runtime() {
    let bus = Bus::start();
    let _service = serve(&bus, FakeFocus(true, "Files", "org.gnome.Nautilus", "Home")).await;
    // Called directly on a runtime worker thread, like the agent does.
    let source = GnomeWindow::with_address(bus.address.clone());
    assert_eq!(
        source.active_window(),
        ActiveWindow::Known {
            app: "Files".into(),
            title: "Home".into()
        }
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn no_focused_window_is_no_window() {
    let bus = Bus::start();
    let _service = serve(&bus, FakeFocus(true, "", "", "")).await;
    assert_eq!(
        ask(GnomeWindow::with_address(bus.address.clone())).await,
        ActiveWindow::NoWindow
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn ok_false_is_unknown() {
    let bus = Bus::start();
    let _service = serve(&bus, FakeFocus(false, "", "", "")).await;
    assert_eq!(
        ask(GnomeWindow::with_address(bus.address.clone())).await,
        ActiveWindow::Unknown
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_missing_service_is_unknown() {
    let bus = Bus::start();
    // The bus is there but nobody owns org.timereel.Focus1.
    let source = GnomeWindow::with_address(bus.address.clone());
    assert_eq!(ask(source).await, ActiveWindow::Unknown);
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unreachable_bus_is_unknown() {
    let source = GnomeWindow::with_address("unix:path=/nonexistent/timereel-test-bus");
    assert_eq!(ask(source).await, ActiveWindow::Unknown);
}

/// The extension must never log: window titles must not reach the journal.
#[test]
fn the_extension_source_has_no_logging_calls() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../extras/gnome-shell-extension/timereel-focus@timereel.org");
    let source = std::fs::read_to_string(dir.join("extension.js")).unwrap();
    for call in [
        "log(",
        "logError(",
        "console.",
        "print(",
        "printerr(",
        "GLib.log",
        "Gio.Notification",
        "Main.notify",
    ] {
        assert!(!source.contains(call), "extension.js must not use {call}");
    }
    // metadata.json is valid JSON and names the right uuid and GNOME 45+.
    let meta = std::fs::read_to_string(dir.join("metadata.json")).unwrap();
    assert!(meta.contains("\"uuid\": \"timereel-focus@timereel.org\""));
    assert!(meta.contains("\"45\""));
}
