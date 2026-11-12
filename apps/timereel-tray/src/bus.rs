//! The tray's side of D-Bus: following the agent and calling it.
//!
//! [`follow`] publishes an [`AgentSnapshot`] on a `watch` channel. It re-reads
//! the agent after every `PropertiesChanged` and `NameOwnerChanged` for the
//! agent's name, and also every few seconds as a safety net. [`Actions`] holds
//! the calls behind the menu entries. Nothing here carries content.

use std::collections::HashMap;
use std::time::Duration;

use futures_util::StreamExt;
use tokio::sync::watch;
use zbus::fdo::{DBusProxy, PropertiesProxy};
use zbus::names::{BusName, InterfaceName};

use zbus::{proxy, Connection};

use crate::model::{AgentSnapshot, State};

/// The bus name of the agent.
pub const AGENT_NAME: &str = "org.timereel.Agent1";
/// The object path of the agent.
pub const AGENT_PATH: &str = "/org/timereel/Agent1";
/// The bus name of the viewer.
pub const VIEWER_NAME: &str = "org.timereel.Viewer1";
/// The object path of the viewer.
pub const VIEWER_PATH: &str = "/org/timereel/Viewer1";
/// The program started by "Open TimeReel" when no viewer is running.
pub const VIEWER_COMMAND: &str = "timereel";

/// How often the agent is re-read even if no signal arrived.
const SAFETY_REFRESH: Duration = Duration::from_secs(5);

/// The calls of `org.timereel.Agent1` that the tray uses.
#[proxy(
    interface = "org.timereel.Agent1",
    default_service = "org.timereel.Agent1",
    default_path = "/org/timereel/Agent1"
)]
trait Agent1 {
    fn pause(&self) -> zbus::Result<()>;
    fn resume(&self) -> zbus::Result<()>;
    fn retry_permission(&self) -> zbus::Result<()>;
}

/// The calls of `org.timereel.Viewer1`.
#[proxy(
    interface = "org.timereel.Viewer1",
    default_service = "org.timereel.Viewer1",
    default_path = "/org/timereel/Viewer1"
)]
trait Viewer1 {
    fn activate(&self) -> zbus::Result<()>;
    fn quit(&self) -> zbus::Result<()>;
}

async fn has_owner(dbus: &DBusProxy<'_>, name: &'static str) -> zbus::Result<bool> {
    Ok(dbus.name_has_owner(BusName::try_from(name)?).await?)
}

/// Reads the agent once.
///
/// # Errors
/// Fails if the bus does not answer. A missing agent is not an error; it gives
/// [`AgentSnapshot::NotRunning`].
pub async fn read_agent(conn: &Connection) -> zbus::Result<AgentSnapshot> {
    let dbus = DBusProxy::new(conn).await?;
    if !has_owner(&dbus, AGENT_NAME).await? {
        return Ok(AgentSnapshot::NotRunning);
    }
    let props = PropertiesProxy::builder(conn)
        .destination(AGENT_NAME)
        .path(AGENT_PATH)
        .build()
        .await?;
    let all: HashMap<String, zbus::OwnedVariant> = match props
        .get_all(InterfaceName::try_from("org.timereel.Agent1")?)
        .await
    {
        Ok(all) => all,
        // The agent left between the two calls.
        Err(zbus::fdo::Error::ServiceUnknown(_)) | Err(zbus::fdo::Error::NameHasNoOwner(_)) => {
            return Ok(AgentSnapshot::NotRunning)
        }
        Err(e) => return Err(e.into()),
    };
    let text = |key: &str| -> String {
        all.get(key)
            .and_then(|v| <&str>::try_from(v).ok())
            .map(str::to_string)
            .unwrap_or_default()
    };
    Ok(AgentSnapshot::Running {
        state: State::parse(&text("State")),
        reason: text("Reason"),
    })
}

/// Follows the agent until `tx` has no receivers left. The first snapshot is
/// published before this function starts waiting, and `tx` is only written
/// when the snapshot changes.
///
/// # Errors
/// Fails if the signal subscriptions cannot be set up.
pub async fn follow(conn: Connection, tx: watch::Sender<AgentSnapshot>) -> zbus::Result<()> {
    let dbus = DBusProxy::new(&conn).await?;
    let props = PropertiesProxy::builder(&conn)
        .destination(AGENT_NAME)
        .path(AGENT_PATH)
        .build()
        .await?;
    let mut owner_changes = dbus
        .receive_name_owner_changed_with_args(&[(0, AGENT_NAME)])
        .await?;
    let mut prop_changes = props.receive_properties_changed().await?;
    let mut tick = tokio::time::interval(SAFETY_REFRESH);

    loop {
        match read_agent(&conn).await {
            Ok(snapshot) => {
                tx.send_if_modified(|current| {
                    let changed = *current != snapshot;
                    if changed {
                        *current = snapshot;
                    }
                    changed
                });
            }
            Err(e) => eprintln!("timereel-tray: could not read the agent: {e}"),
        }
        tokio::select! {
            _ = owner_changes.next() => {}
            _ = prop_changes.next() => {}
            _ = tick.tick() => {}
            () = tx.closed() => return Ok(()),
        }
    }
}

/// The calls behind the menu entries.
#[derive(Clone)]
pub struct Actions {
    conn: Connection,
}

impl Actions {
    /// Makes the actions on `conn`.
    pub fn new(conn: Connection) -> Actions {
        Actions { conn }
    }

    /// Calls `Pause` on the agent.
    ///
    /// # Errors
    /// Fails if the agent is not on the bus or refuses.
    pub async fn pause(&self) -> zbus::Result<()> {
        Agent1Proxy::new(&self.conn).await?.pause().await
    }

    /// Calls `Resume` on the agent.
    ///
    /// # Errors
    /// Fails if the agent is not on the bus or refuses.
    pub async fn resume(&self) -> zbus::Result<()> {
        Agent1Proxy::new(&self.conn).await?.resume().await
    }

    /// Calls `RetryPermission` on the agent.
    ///
    /// # Errors
    /// Fails if the agent is not on the bus or refuses.
    pub async fn retry_permission(&self) -> zbus::Result<()> {
        Agent1Proxy::new(&self.conn).await?.retry_permission().await
    }

    /// Brings the viewer to the front if it runs, otherwise starts `timereel`.
    ///
    /// # Errors
    /// Fails if the viewer cannot be called or started.
    pub async fn open_viewer(&self) -> Result<(), String> {
        let dbus = DBusProxy::new(&self.conn)
            .await
            .map_err(|e| e.to_string())?;
        if has_owner(&dbus, VIEWER_NAME)
            .await
            .map_err(|e| e.to_string())?
        {
            return Viewer1Proxy::new(&self.conn)
                .await
                .map_err(|e| e.to_string())?
                .activate()
                .await
                .map_err(|e| e.to_string());
        }
        let mut child = tokio::process::Command::new(VIEWER_COMMAND)
            .spawn()
            .map_err(|e| format!("could not start {VIEWER_COMMAND}: {e}"))?;
        // Reap the child when it ends, so that no zombie is left behind.
        tokio::spawn(async move {
            let _ = child.wait().await;
        });
        Ok(())
    }

    /// Asks the viewer to quit, if there is one. It never calls the agent.
    ///
    /// # Errors
    /// Fails if the viewer is on the bus but does not accept the call.
    pub async fn quit_viewer(&self) -> Result<(), String> {
        let dbus = DBusProxy::new(&self.conn)
            .await
            .map_err(|e| e.to_string())?;
        if !has_owner(&dbus, VIEWER_NAME)
            .await
            .map_err(|e| e.to_string())?
        {
            return Ok(());
        }
        Viewer1Proxy::new(&self.conn)
            .await
            .map_err(|e| e.to_string())?
            .quit()
            .await
            .map_err(|e| e.to_string())
    }
}
