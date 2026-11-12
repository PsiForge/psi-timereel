//! `timereel-agent`: the capture agent, run as a systemd user service.

use std::sync::Arc;

use timereel_agent::clock::SystemClock;
use timereel_agent::cycle::CycleRunner;
use timereel_agent::runtime::Core;
use timereel_agent::sources::screen_and_window;
use timereel_agent::{logging, logind, service, sys};
use timereel_capture::lock::DbusLockSource;
use timereel_store::Store;

fn main() -> std::process::ExitCode {
    if std::env::args().nth(1).as_deref() == Some("--version") {
        println!("{} {}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"));
        return std::process::ExitCode::SUCCESS;
    }
    let target = logging::init();
    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        target,
        "agent starting"
    );
    if let Err(e) = sys::disable_dumping() {
        // Not fatal, but the owner should know that the hardening is missing.
        tracing::warn!(error = %e, "could not turn off core dumps");
    }

    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            tracing::error!(error = %e, "could not start the async runtime");
            return std::process::ExitCode::FAILURE;
        }
    };
    match runtime.block_on(run()) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(message) => {
            tracing::error!("{message}");
            eprintln!("timereel-agent: {message}");
            std::process::ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<(), String> {
    let store = Store::at_default_location().map_err(|e| format!("no data directory: {e}"))?;
    let bus = zbus::Connection::session()
        .await
        .map_err(|e| format!("cannot connect to the session bus: {e}"))?;
    // The system bus serves logind (suspend and lock detection). Without it
    // the schedule is still right, because it is checked against the wall
    // clock, and the lock sources fall back to the ScreenSaver services.
    let system = match zbus::Connection::system().await {
        Ok(system) => Some(system),
        Err(e) => {
            tracing::warn!(error = %e, "no system bus, not following suspend or logind lock");
            None
        }
    };
    let lock = DbusLockSource::start(system.clone(), Some(bus.clone())).await;
    let session = timereel_capture::detect_session();
    tracing::info!(session = ?session, "session detected");
    let (screen, window) = screen_and_window(session);
    let runner = CycleRunner::new(screen, window, Box::new(lock), Box::new(store.clone()));
    let (core, handle) = Core::new(Arc::new(SystemClock), runner, store);

    service::serve(&bus, handle.clone())
        .await
        .map_err(|e| format!("cannot serve {}: {e}", service::BUS_NAME))?;

    if let Some(system) = system {
        let h = handle.clone();
        tokio::spawn(async move {
            if let Err(e) = logind::watch_sleep(&system, move || h.notify_resumed()).await {
                tracing::warn!(error = %e, "not following logind PrepareForSleep");
            }
        });
    }

    let stopper = handle.clone();
    tokio::spawn(async move {
        wait_for_stop_signal().await;
        stopper.shutdown();
    });
    tracing::info!("agent running");
    core.run().await;
    Ok(())
}

/// Waits for SIGTERM (systemd stop) or SIGINT.
async fn wait_for_stop_signal() {
    use tokio::signal::unix::{signal, SignalKind};
    let (Ok(mut term), Ok(mut int)) = (
        signal(SignalKind::terminate()),
        signal(SignalKind::interrupt()),
    ) else {
        tracing::warn!("cannot listen for stop signals");
        return std::future::pending().await;
    };
    tokio::select! {
        _ = term.recv() => {}
        _ = int.recv() => {}
    }
}
