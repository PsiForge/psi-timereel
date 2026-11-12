//! The active window on GNOME Wayland, read from the bundled GNOME Shell
//! extension `timereel-focus@timereel.org` (ADR 0009).
//!
//! The extension exports `org.timereel.Focus1.GetFocus() -> (ok, app_name,
//! wm_class, title)` on the session bus. [`GnomeWindow`] calls it:
//!
//! * `ok = true` with all three strings empty: no window has the focus
//!   ([`ActiveWindow::NoWindow`]);
//! * `ok = true` with an app: [`ActiveWindow::Known`]. The app is `app_name`
//!   (what the desktop shows), or `wm_class` if the name is empty;
//! * anything else (no extension, a call that fails or takes too long,
//!   `ok = false`): [`ActiveWindow::Unknown`]. One warning is logged the first
//!   time, and never any content.
//!
//! [`ActiveWindowSource::active_window`] is synchronous and the agent calls it
//! from inside its own async runtime, so each call runs on a short-lived
//! helper thread with its own runtime and bus connection (a runtime cannot be
//! entered from within another one). The agent calls it twice every few
//! minutes, so the connection cost does not matter.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use zbus::{connection, Proxy};

use crate::{ActiveWindow, ActiveWindowSource};

/// Bus name, object path and interface of the extension.
pub const BUS_NAME: &str = "org.timereel.Focus1";
/// Object path of the extension.
pub const OBJECT_PATH: &str = "/org/timereel/Focus1";
/// How long one call may take (connecting and answering) before the app is
/// reported as unknown.
const CALL_TIMEOUT: Duration = Duration::from_secs(2);

/// An [`ActiveWindowSource`] that asks the GNOME Shell extension.
#[derive(Debug, Default)]
pub struct GnomeWindow {
    /// Address of the bus to use; `None` is the user's session bus.
    address: Option<String>,
    warned: AtomicBool,
}

impl GnomeWindow {
    /// A source on the user's session bus (`DBUS_SESSION_BUS_ADDRESS`). It
    /// does not talk to D-Bus until the first call.
    pub fn new() -> Self {
        Self::default()
    }

    /// A source on the bus at `address`. Tests use it with a private bus.
    pub fn with_address(address: impl Into<String>) -> Self {
        Self {
            address: Some(address.into()),
            warned: AtomicBool::new(false),
        }
    }

    fn query(&self) -> Result<ActiveWindow, Failure> {
        let address = self.address.clone();
        // A helper thread, because the caller is usually inside a runtime.
        std::thread::scope(|scope| {
            scope
                .spawn(move || {
                    let rt = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .map_err(|_| Failure::Runtime)?;
                    let result = rt.block_on(async {
                        tokio::time::timeout(CALL_TIMEOUT, get_focus(address.as_deref()))
                            .await
                            .unwrap_or(Err(Failure::TimedOut))
                    });
                    rt.shutdown_background();
                    result
                })
                .join()
                .unwrap_or(Err(Failure::Runtime))
        })
    }
}

/// Why the extension could not be asked. Fixed names only, so they are safe
/// to log.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Failure {
    Runtime,
    Bus,
    TimedOut,
    /// The extension is not running, or the call failed.
    Call,
}

impl Failure {
    fn name(self) -> &'static str {
        match self {
            Failure::Runtime => "runtime",
            Failure::Bus => "bus",
            Failure::TimedOut => "timeout",
            Failure::Call => "call",
        }
    }
}

async fn get_focus(address: Option<&str>) -> Result<ActiveWindow, Failure> {
    let conn = match address {
        Some(a) => connection::Builder::address(a)
            .build()
            .await
            .map_err(|_| Failure::Bus)?,
        None => zbus::Connection::session()
            .await
            .map_err(|_| Failure::Bus)?,
    };
    let proxy = Proxy::new(&conn, BUS_NAME, OBJECT_PATH, BUS_NAME)
        .await
        .map_err(|_| Failure::Bus)?;
    let (ok, app_name, wm_class, title): (bool, String, String, String) = proxy
        .call("GetFocus", &())
        .await
        .map_err(|_| Failure::Call)?;
    Ok(interpret(ok, app_name, wm_class, title))
}

/// Turns the reply of `GetFocus` into an [`ActiveWindow`].
fn interpret(ok: bool, app_name: String, wm_class: String, title: String) -> ActiveWindow {
    if !ok {
        return ActiveWindow::Unknown;
    }
    let app = if app_name.is_empty() {
        wm_class
    } else {
        app_name
    };
    match (app.is_empty(), title.is_empty()) {
        (false, _) => ActiveWindow::Known { app, title },
        (true, true) => ActiveWindow::NoWindow,
        // A title without any app name cannot be matched against the
        // excluded list, so the app is unknown.
        (true, false) => ActiveWindow::Unknown,
    }
}

impl ActiveWindowSource for GnomeWindow {
    fn active_window(&self) -> ActiveWindow {
        match self.query() {
            Ok(window) => window,
            Err(failure) => {
                if !self.warned.swap(true, Ordering::Relaxed) {
                    tracing::warn!(
                        why = failure.name(),
                        "the TimeReel GNOME extension did not answer: the active app is unknown"
                    );
                }
                ActiveWindow::Unknown
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &str) -> String {
        v.to_string()
    }

    #[test]
    fn interprets_the_reply() {
        assert_eq!(
            interpret(true, s("Firefox"), s("firefox"), s("Inbox")),
            ActiveWindow::Known {
                app: s("Firefox"),
                title: s("Inbox")
            }
        );
        // Without an app name the window class is used.
        assert_eq!(
            interpret(true, s(""), s("KeePassXC"), s("")),
            ActiveWindow::Known {
                app: s("KeePassXC"),
                title: s("")
            }
        );
        assert_eq!(interpret(true, s(""), s(""), s("")), ActiveWindow::NoWindow);
        assert_eq!(interpret(true, s(""), s(""), s("T")), ActiveWindow::Unknown);
        assert_eq!(
            interpret(false, s("Firefox"), s("x"), s("T")),
            ActiveWindow::Unknown
        );
    }
}
