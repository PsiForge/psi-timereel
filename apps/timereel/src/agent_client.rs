//! The viewer's side of `org.timereel.Agent1`: the calls the Settings screen
//! makes. [`AgentControl`] is a trait so that tests can use a fake; the real
//! implementation is [`DbusAgent`].
//!
//! The calls block the caller for a short time. Each one first checks that
//! the agent owns its bus name, so a missing agent is reported at once
//! ([`AgentError::NotRunning`]); a running agent that does not answer within
//! [`CALL_TIMEOUT`] gives [`AgentError::Failed`].

use std::time::Duration;

use zbus::{Connection, Proxy};

/// The bus name of the agent.
pub const AGENT_NAME: &str = "org.timereel.Agent1";
/// The object path of the agent.
pub const AGENT_PATH: &str = "/org/timereel/Agent1";
/// The interface of the agent.
pub const AGENT_INTERFACE: &str = "org.timereel.Agent1";
/// How long a call waits for the agent.
pub const CALL_TIMEOUT: Duration = Duration::from_secs(5);

/// Why a call to the agent failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AgentError {
    /// The agent is not on the bus (or there is no session bus).
    #[error("The agent is not running")]
    NotRunning,
    /// The agent answered with an error, or did not answer.
    #[error("{0}")]
    Failed(String),
}

/// The agent calls used by the Settings screen.
pub trait AgentControl {
    /// `ReloadConfig()`: the agent re-reads the config. An invalid config is
    /// [`AgentError::Failed`] with the agent's text.
    ///
    /// # Errors
    /// See [`AgentError`].
    fn reload_config(&self) -> Result<(), AgentError>;
    /// `RunRetention()`: returns the number of captures deleted.
    ///
    /// # Errors
    /// See [`AgentError`].
    fn run_retention(&self) -> Result<u32, AgentError>;
    /// `Pause()` (`true`) or `Resume()` (`false`).
    ///
    /// # Errors
    /// See [`AgentError`].
    fn set_paused(&self, paused: bool) -> Result<(), AgentError>;
}

/// Calls the real agent over the session bus.
#[derive(Debug, Default, Clone, Copy)]
pub struct DbusAgent;

impl DbusAgent {
    fn call<T>(&self, method: &'static str) -> Result<T, AgentError>
    where
        T: for<'d> zbus::zvariant::DynamicDeserialize<'d> + zbus::zvariant::Type,
    {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| AgentError::Failed(format!("Cannot start the D-Bus client: {e}")))?;
        runtime.block_on(async {
            tokio::time::timeout(CALL_TIMEOUT, call_agent::<T>(method))
                .await
                .unwrap_or_else(|_| Err(AgentError::Failed("The agent did not answer".into())))
        })
    }
}

async fn call_agent<T>(method: &'static str) -> Result<T, AgentError>
where
    T: for<'d> zbus::zvariant::DynamicDeserialize<'d> + zbus::zvariant::Type,
{
    let connection = Connection::session()
        .await
        .map_err(|_| AgentError::NotRunning)?;
    let dbus = zbus::fdo::DBusProxy::new(&connection)
        .await
        .map_err(|_| AgentError::NotRunning)?;
    let name = zbus::names::BusName::try_from(AGENT_NAME)
        .map_err(|e| AgentError::Failed(e.to_string()))?;
    if !dbus.name_has_owner(name).await.unwrap_or(false) {
        return Err(AgentError::NotRunning);
    }
    let proxy = Proxy::new(&connection, AGENT_NAME, AGENT_PATH, AGENT_INTERFACE)
        .await
        .map_err(|e| AgentError::Failed(e.to_string()))?;
    proxy
        .call::<_, _, T>(method, &())
        .await
        .map_err(|e| match e {
            // The agent left between the two calls.
            zbus::Error::MethodError(name, _, _)
                if name.as_str() == "org.freedesktop.DBus.Error.ServiceUnknown"
                    || name.as_str() == "org.freedesktop.DBus.Error.NameHasNoOwner" =>
            {
                AgentError::NotRunning
            }
            other => AgentError::Failed(other.to_string()),
        })
}

impl AgentControl for DbusAgent {
    fn reload_config(&self) -> Result<(), AgentError> {
        let text: String = self.call("ReloadConfig")?;
        if text.is_empty() {
            Ok(())
        } else {
            Err(AgentError::Failed(text))
        }
    }

    fn run_retention(&self) -> Result<u32, AgentError> {
        self.call("RunRetention")
    }

    fn set_paused(&self, paused: bool) -> Result<(), AgentError> {
        self.call(if paused { "Pause" } else { "Resume" })
    }
}
