//! `timereel-tray`: the tray icon with the agent state and a few controls.

use std::sync::Arc;

use ksni::TrayMethods;
use timereel_tray::bus::{follow, Actions};
use timereel_tray::model::AgentSnapshot;
use timereel_tray::tray::TimeReelTray;
use tokio::sync::{watch, Notify};
use zbus::Connection;

async fn run() -> Result<(), String> {
    let conn = Connection::session()
        .await
        .map_err(|e| format!("cannot connect to the session bus: {e}"))?;

    let quit = Arc::new(Notify::new());
    let tray = TimeReelTray::new(Actions::new(conn.clone()), Arc::clone(&quit));
    // The panel may not be up yet at login; the icon appears when it is.
    let handle = tray
        .assume_sni_available(true)
        .spawn()
        .await
        .map_err(|e| format!("cannot start the tray icon: {e}"))?;

    let (tx, mut rx) = watch::channel(AgentSnapshot::NotRunning);
    let mut follower = tokio::spawn(follow(conn, tx));

    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .map_err(|e| format!("cannot listen for SIGTERM: {e}"))?;
    loop {
        tokio::select! {
            changed = rx.changed() => {
                if changed.is_err() {
                    return Err("lost the agent follower".to_string());
                }
                let snapshot = rx.borrow_and_update().clone();
                handle.update(move |t: &mut TimeReelTray| t.set_snapshot(&snapshot)).await;
            }
            () = quit.notified() => break,
            _ = tokio::signal::ctrl_c() => break,
            _ = terminate.recv() => break,
            result = &mut follower => {
                return match result {
                    Ok(Ok(())) => Err("the agent follower stopped".to_string()),
                    Ok(Err(e)) => Err(format!("the agent follower failed: {e}")),
                    Err(e) => Err(format!("the agent follower crashed: {e}")),
                };
            }
        }
    }
    handle.shutdown().await;
    Ok(())
}

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() {
    if let Err(message) = run().await {
        eprintln!("timereel-tray: {message}");
        std::process::exit(1);
    }
}
