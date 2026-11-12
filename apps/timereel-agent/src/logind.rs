//! Watches logind's `PrepareForSleep` signal on the system bus, so the agent
//! reschedules after a suspend (architecture section 2.2).

use futures_util::StreamExt;
use zbus::{proxy, Connection};

/// The logind manager, only the part the agent needs.
#[proxy(
    interface = "org.freedesktop.login1.Manager",
    default_service = "org.freedesktop.login1",
    default_path = "/org/freedesktop/login1"
)]
trait Login1Manager {
    /// Sent with `true` before the machine suspends and with `false` after it
    /// has woken up.
    #[zbus(signal)]
    fn prepare_for_sleep(&self, start: bool) -> zbus::Result<()>;
}

/// Follows `PrepareForSleep` on `connection` until the signal stream ends.
/// Every wake-up (`start == false`) calls `on_resume`; the agent passes
/// [`AgentHandle::notify_resumed`](crate::runtime::AgentHandle::notify_resumed).
///
/// # Errors
/// Fails if the proxy or the signal subscription cannot be created, for
/// example when the system bus is not available.
pub async fn watch_sleep(connection: &Connection, on_resume: impl Fn()) -> zbus::Result<()> {
    let proxy = Login1ManagerProxy::new(connection).await?;
    let mut signals = proxy.receive_prepare_for_sleep().await?;
    while let Some(signal) = signals.next().await {
        match signal.args() {
            Ok(args) if !args.start => on_resume(),
            Ok(_) => tracing::debug!("system is going to sleep"),
            Err(_) => tracing::warn!("unreadable PrepareForSleep signal"),
        }
    }
    Ok(())
}
