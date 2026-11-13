//! Follows the desktop lock state (T12's [`DbusLockSource`]) for the viewer,
//! which locks when the desktop session locks.

use std::future::Future;
use std::sync::mpsc::{channel, Receiver};
use std::sync::{Arc, Mutex};

use futures_util::StreamExt;
use timereel_capture::lock::DbusLockSource;
use zbus::Connection;

type SharedContext = Arc<Mutex<Option<egui::Context>>>;

/// Lock changes of the desktop session, delivered to the UI thread.
pub struct LockWatch {
    changes: Receiver<bool>,
    ctx: SharedContext,
    // Dropping the runtime stops the background task.
    _runtime: tokio::runtime::Runtime,
}

impl LockWatch {
    /// Follows logind on the system bus and the ScreenSaver services on the
    /// session bus. A bus that cannot be reached is left out.
    ///
    /// # Errors
    /// Fails if the background thread cannot be started.
    pub fn start_default() -> std::io::Result<Self> {
        Self::start(|| async {
            (
                Connection::system().await.ok(),
                Connection::session().await.ok(),
            )
        })
    }

    /// Follows the buses that `connect` returns (system, session).
    ///
    /// # Errors
    /// Fails if the background thread cannot be started.
    pub fn start<F, Fut>(connect: F) -> std::io::Result<Self>
    where
        F: FnOnce() -> Fut + Send + 'static,
        Fut: Future<Output = (Option<Connection>, Option<Connection>)> + Send,
    {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .thread_name("timereel-lockwatch")
            .enable_all()
            .build()?;
        let (tx, changes) = channel();
        let ctx: SharedContext = Arc::default();
        let wake = ctx.clone();
        runtime.spawn(async move {
            let (system, session) = connect().await;
            let source = DbusLockSource::start(system, session).await;
            let mut stream = Box::pin(source.changes());
            while let Some(locked) = stream.next().await {
                if tx.send(locked).is_err() {
                    break;
                }
                if let Ok(ctx) = wake.lock() {
                    if let Some(ctx) = ctx.as_ref() {
                        ctx.request_repaint();
                    }
                }
            }
        });
        Ok(Self {
            changes,
            ctx,
            _runtime: runtime,
        })
    }

    /// Lets a lock change wake up the UI.
    pub fn attach(&self, ctx: &egui::Context) {
        if let Ok(mut slot) = self.ctx.lock() {
            *slot = Some(ctx.clone());
        }
    }

    /// True if the desktop session locked since the last call.
    pub fn session_locked(&self) -> bool {
        // Drain every change, so an old one is not seen in a later frame.
        let mut locked = false;
        for change in self.changes.try_iter() {
            locked |= change;
        }
        locked
    }
}
