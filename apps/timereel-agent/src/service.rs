//! The `org.timereel.Agent1` D-Bus service (architecture section 3.1).
//!
//! The interface is thin: every method forwards to the main loop through an
//! [`AgentHandle`], and the properties are read from the status the loop
//! publishes. A small task turns status changes into `PropertiesChanged`
//! signals. Nothing here carries titles, images or keys.

use zbus::object_server::SignalEmitter;
use zbus::{fdo, interface, Connection};

use crate::runtime::{AgentHandle, Status};

/// The well-known bus name of the agent.
pub const BUS_NAME: &str = "org.timereel.Agent1";
/// The object path of the agent.
pub const OBJECT_PATH: &str = "/org/timereel/Agent1";

/// The object behind `org.timereel.Agent1`.
pub struct AgentInterface {
    handle: AgentHandle,
}

fn failed(text: String) -> fdo::Error {
    fdo::Error::Failed(text)
}

#[interface(name = "org.timereel.Agent1")]
impl AgentInterface {
    /// `recording`, `paused`, `error`, `needs_permission` or `not_set_up`.
    #[zbus(property)]
    async fn state(&self) -> String {
        self.handle.status().state.as_str().to_string()
    }

    /// A short reason for users, empty when there is nothing to say.
    #[zbus(property)]
    async fn reason(&self) -> String {
        self.handle.status().reason
    }

    /// Unix time in ms of the last capture, or 0.
    #[zbus(property)]
    async fn last_capture_time(&self) -> i64 {
        self.handle.status().last_capture_ms
    }

    /// `paused`, `locked`, `excluded`, `app-unknown`, or empty.
    #[zbus(property)]
    async fn last_skip_reason(&self) -> String {
        self.handle.status().last_skip
    }

    /// Saves `paused = true` to the config and stops capturing at once.
    async fn pause(&self) -> fdo::Result<()> {
        self.handle.set_paused(true).await.map_err(failed)
    }

    /// Saves `paused = false` to the config and resumes capturing.
    async fn resume(&self) -> fdo::Result<()> {
        self.handle.set_paused(false).await.map_err(failed)
    }

    /// Re-reads and validates the config and reschedules the next tick.
    /// Returns an error text if the config is invalid, else an empty string.
    async fn reload_config(&self) -> fdo::Result<String> {
        self.handle.reload_config().await.map_err(failed)
    }

    /// Runs retention now and returns the number of captures deleted.
    async fn run_retention(&self) -> fdo::Result<u32> {
        self.handle.run_retention().await.map_err(failed)
    }

    /// On Wayland, asks for portal permission again and captures at once so
    /// the portal dialog is shown. Without a permission step it only causes
    /// an early capture.
    async fn retry_permission(&self) -> fdo::Result<()> {
        self.handle.retry_permission().await.map_err(failed)
    }
}

/// Registers the interface on `connection`, starts the task that emits
/// `PropertiesChanged`, and claims [`BUS_NAME`].
///
/// # Errors
/// Fails if the object cannot be registered or the name is already owned by
/// another process.
pub async fn serve(connection: &Connection, handle: AgentHandle) -> zbus::Result<()> {
    connection
        .object_server()
        .at(
            OBJECT_PATH,
            AgentInterface {
                handle: handle.clone(),
            },
        )
        .await?;
    let iface_ref = connection
        .object_server()
        .interface::<_, AgentInterface>(OBJECT_PATH)
        .await?;
    connection.request_name(BUS_NAME).await?;

    let mut rx = handle.subscribe();
    let mut previous: Status = handle.status();
    let conn = connection.clone();
    tokio::spawn(async move {
        // Hold the connection so that it lives as long as the loop is running.
        let _conn = conn;
        loop {
            // Stop when the loop has dropped its sender.
            if rx.changed().await.is_err() {
                break;
            }
            let current = rx.borrow_and_update().clone();
            let iface = iface_ref.get().await;
            let emitter: &SignalEmitter<'_> = iface_ref.signal_emitter();
            let mut result = Ok(());
            if current.state != previous.state {
                result = result.and(iface.state_changed(emitter).await);
            }
            if current.reason != previous.reason {
                result = result.and(iface.reason_changed(emitter).await);
            }
            if current.last_capture_ms != previous.last_capture_ms {
                result = result.and(iface.last_capture_time_changed(emitter).await);
            }
            if current.last_skip != previous.last_skip {
                result = result.and(iface.last_skip_reason_changed(emitter).await);
            }
            if result.is_err() {
                tracing::warn!("could not emit PropertiesChanged");
            }
            previous = current;
        }
    });
    Ok(())
}
