//! Lock detection over D-Bus (architecture section 5).
//!
//! [`DbusLockSource`] asks, in this order:
//!
//! 1. logind: `LockedHint` of the current session
//!    (`/org/freedesktop/login1/session/auto`), on the system bus;
//! 2. `org.gnome.ScreenSaver.GetActive`, on the session bus;
//! 3. `org.freedesktop.ScreenSaver.GetActive`, on the session bus.
//!
//! The first one that answers is used. If none does, one warning is logged
//! and the screen counts as unlocked.
//!
//! [`LockSource::is_locked`] is synchronous, so the source keeps the last
//! known state and a background task updates it from the change signal of
//! the chosen service (`PropertiesChanged` or `ActiveChanged`). The same
//! changes are available as a stream through [`DbusLockSource::changes`], for
//! the viewer's lock-on-session-lock.
//!
//! The bus connections are passed in, so tests can use a private bus.

use std::time::Duration;

use futures_util::{Stream, StreamExt};
use tokio::sync::watch;
use zbus::{Connection, Proxy};

use crate::LockSource;

const LOGIND_NAME: &str = "org.freedesktop.login1";
const LOGIND_SESSION_IFACE: &str = "org.freedesktop.login1.Session";
/// logind resolves `auto` to the session of the caller.
const LOGIND_SESSION_AUTO: &str = "/org/freedesktop/login1/session/auto";
const GNOME: (&str, &str) = ("org.gnome.ScreenSaver", "/org/gnome/ScreenSaver");
const FREEDESKTOP: (&str, &str) = (
    "org.freedesktop.ScreenSaver",
    "/org/freedesktop/ScreenSaver",
);
/// How long one source may take to answer before the next one is tried.
const ANSWER_TIMEOUT: Duration = Duration::from_secs(2);

/// A [`LockSource`] backed by logind and the ScreenSaver services.
#[derive(Debug)]
pub struct DbusLockSource {
    state: watch::Sender<bool>,
    task: Option<tokio::task::JoinHandle<()>>,
}

impl DbusLockSource {
    /// Finds the first source that answers and starts following it. Must be
    /// called inside a tokio runtime.
    ///
    /// `system` is used for logind, `session` for the ScreenSaver services;
    /// pass `None` for a bus that is not available.
    pub async fn start(system: Option<Connection>, session: Option<Connection>) -> Self {
        let (state, _) = watch::channel(false);
        let found = find_source(system.as_ref(), session.as_ref()).await;
        let task = match found {
            Some((initial, follow)) => {
                state.send_replace(initial);
                let tx = state.clone();
                Some(tokio::spawn(async move {
                    follow.run(tx).await;
                    tracing::warn!("lock state source went away; keeping the last state");
                }))
            }
            None => {
                tracing::warn!(
                    "no lock source answered (logind, GNOME and freedesktop ScreenSaver): \
                     treating the screen as unlocked"
                );
                None
            }
        };
        Self { state, task }
    }

    /// A stream of lock changes: `true` when the screen locks, `false` when
    /// it unlocks. It only yields real changes, not the current state.
    pub fn changes(&self) -> impl Stream<Item = bool> + Send + 'static {
        let rx = self.state.subscribe();
        futures_util::stream::unfold(rx, |mut rx| async move {
            rx.changed().await.ok()?;
            let value = *rx.borrow_and_update();
            Some((value, rx))
        })
    }
}

impl Drop for DbusLockSource {
    fn drop(&mut self) {
        if let Some(task) = &self.task {
            task.abort();
        }
    }
}

impl LockSource for DbusLockSource {
    fn is_locked(&self) -> bool {
        *self.state.borrow()
    }
}

/// How the chosen source reports changes.
enum Follow {
    /// logind: `PropertiesChanged` of `LockedHint`.
    Hint(Proxy<'static>),
    /// ScreenSaver: the `ActiveChanged(b)` signal.
    Signal(Proxy<'static>),
}

impl Follow {
    async fn run(self, tx: watch::Sender<bool>) {
        match self {
            Follow::Hint(proxy) => {
                let mut changes = proxy.receive_property_changed::<bool>("LockedHint").await;
                while let Some(change) = changes.next().await {
                    match change.get().await {
                        Ok(v) => publish(&tx, v),
                        Err(e) => tracing::warn!(error = %e, "unreadable LockedHint change"),
                    }
                }
            }
            Follow::Signal(proxy) => {
                let Ok(mut signals) = proxy.receive_signal("ActiveChanged").await else {
                    return;
                };
                while let Some(msg) = signals.next().await {
                    match msg.body().deserialize::<bool>() {
                        Ok(v) => publish(&tx, v),
                        Err(_) => tracing::warn!("unreadable ActiveChanged signal"),
                    }
                }
            }
        }
    }
}

fn publish(tx: &watch::Sender<bool>, value: bool) {
    tx.send_if_modified(|current| {
        let changed = *current != value;
        *current = value;
        changed
    });
}

async fn find_source(
    system: Option<&Connection>,
    session: Option<&Connection>,
) -> Option<(bool, Follow)> {
    if let Some(conn) = system {
        if let Some(found) = tokio::time::timeout(ANSWER_TIMEOUT, logind(conn))
            .await
            .ok()
            .flatten()
        {
            return Some(found);
        }
        tracing::debug!("logind did not answer");
    }
    if let Some(conn) = session {
        for (name, path) in [GNOME, FREEDESKTOP] {
            if let Some(found) = tokio::time::timeout(ANSWER_TIMEOUT, screensaver(conn, name, path))
                .await
                .ok()
                .flatten()
            {
                return Some(found);
            }
            tracing::debug!(service = name, "ScreenSaver did not answer");
        }
    }
    None
}

async fn logind(conn: &Connection) -> Option<(bool, Follow)> {
    // The signals come from the real session path, not from `auto`, so look
    // the real path up. Without it (a minimal service) `auto` is used.
    let path = real_session_path(conn)
        .await
        .unwrap_or_else(|| LOGIND_SESSION_AUTO.to_string());
    let proxy = Proxy::new(conn, LOGIND_NAME, path, LOGIND_SESSION_IFACE)
        .await
        .ok()?;
    let value: bool = proxy.get_property("LockedHint").await.ok()?;
    Some((value, Follow::Hint(proxy)))
}

async fn real_session_path(conn: &Connection) -> Option<String> {
    let auto = Proxy::new(conn, LOGIND_NAME, LOGIND_SESSION_AUTO, LOGIND_SESSION_IFACE)
        .await
        .ok()?;
    let id: String = auto.get_property("Id").await.ok()?;
    let manager = Proxy::new(
        conn,
        LOGIND_NAME,
        "/org/freedesktop/login1",
        "org.freedesktop.login1.Manager",
    )
    .await
    .ok()?;
    let path: zbus::OwnedObjectPath = manager.call("GetSession", &(id,)).await.ok()?;
    Some(path.to_string())
}

async fn screensaver(
    conn: &Connection,
    name: &'static str,
    path: &'static str,
) -> Option<(bool, Follow)> {
    let proxy = Proxy::new(conn, name, path, name).await.ok()?;
    let active: bool = proxy.call("GetActive", &()).await.ok()?;
    Some((active, Follow::Signal(proxy)))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use std::io;
    use std::sync::{Arc, Mutex};

    use super::*;

    #[derive(Clone, Default)]
    struct Buf(Arc<Mutex<Vec<u8>>>);

    impl io::Write for Buf {
        fn write(&mut self, data: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(data);
            Ok(data.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn no_source_answering_warns_once_and_is_unlocked() {
        let buf = Buf::default();
        let sink = buf.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(move || sink.clone())
            .with_ansi(false)
            .finish();
        let _guard = tracing::subscriber::set_default(subscriber);

        let source = DbusLockSource::start(None, None).await;
        for _ in 0..3 {
            assert!(!source.is_locked());
        }
        let log = String::from_utf8(buf.0.lock().unwrap().clone()).unwrap();
        assert_eq!(log.matches("WARN").count(), 1, "log: {log}");
        assert!(log.contains("no lock source answered"));
    }
}
