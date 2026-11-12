//! The real portal: ashpd for D-Bus, pipewire-rs for the frames.
//!
//! The agent calls `ScreenSource::capture` from inside its own async runtime,
//! so every blocking step here runs on a short-lived helper thread
//! ([`off_runtime`]); a runtime cannot be entered from within another one.

use std::os::fd::OwnedFd;
use std::time::Duration;

use ashpd::desktop::screencast::{
    CursorMode, Screencast, SelectSourcesOptions, SourceType, Streams,
};
use ashpd::desktop::{PersistMode, Session};
use ashpd::Error as AshpdError;
use timereel_image::RgbaImage;
use tokio::runtime::{Builder, Runtime};

use super::{pipewire_grab, Portal, PortalError, PortalSession, StreamInfo};

/// A tokio runtime that is shut down without blocking when it is dropped
/// (dropping a runtime normally panics inside an async context).
struct Rt(Option<Runtime>);

impl Rt {
    fn get(&self) -> &Runtime {
        // `None` only after drop.
        self.0.as_ref().expect("runtime is present until drop")
    }
}

impl Drop for Rt {
    fn drop(&mut self) {
        if let Some(rt) = self.0.take() {
            rt.shutdown_background();
        }
    }
}

/// Runs `f` on a helper thread and waits for it.
fn off_runtime<T: Send>(f: impl FnOnce() -> T + Send) -> Result<T, PortalError> {
    std::thread::scope(|scope| {
        scope.spawn(f).join().map_err(|_| {
            tracing::error!("the portal helper thread panicked");
            PortalError::Failed
        })
    })
}

fn map_error(error: &AshpdError) -> PortalError {
    match error {
        AshpdError::Response(ashpd::desktop::ResponseError::Cancelled) => PortalError::Denied,
        other => {
            // The variant names carry no user content; the details might.
            tracing::warn!(kind = ?std::mem::discriminant(other), "portal call failed");
            PortalError::Failed
        }
    }
}

/// The ScreenCast portal over the session bus.
pub struct ScreencastPortal {
    rt: std::sync::Arc<Rt>,
}

impl ScreencastPortal {
    /// Creates the portal client. It does not talk to D-Bus yet.
    ///
    /// # Errors
    /// Fails if the helper runtime cannot be built.
    pub fn new() -> std::io::Result<Self> {
        let rt = Builder::new_current_thread().enable_all().build()?;
        Ok(Self {
            rt: std::sync::Arc::new(Rt(Some(rt))),
        })
    }
}

impl Portal for ScreencastPortal {
    fn version(&self) -> Result<u32, PortalError> {
        let rt = &self.rt;
        off_runtime(|| {
            rt.get().block_on(async {
                let proxy = Screencast::new().await.map_err(|e| map_error(&e))?;
                Ok(proxy.version())
            })
        })?
    }

    fn start(
        &self,
        restore_token: Option<&str>,
        timeout: Duration,
    ) -> Result<Box<dyn PortalSession>, PortalError> {
        let rt = std::sync::Arc::clone(&self.rt);
        let started = off_runtime(|| rt.get().block_on(start_session(restore_token, timeout)))??;
        Ok(Box::new(RealSession {
            rt: std::sync::Arc::clone(&self.rt),
            proxy: started.proxy,
            session: Some(started.session),
            fd: Some(started.fd),
            streams: started.streams,
            restore_token: started.restore_token,
        }))
    }
}

struct Started {
    proxy: Screencast,
    session: Session<Screencast>,
    fd: OwnedFd,
    streams: Vec<StreamInfo>,
    restore_token: Option<String>,
}

async fn start_session(
    restore_token: Option<&str>,
    timeout: Duration,
) -> Result<Started, PortalError> {
    let deadline = tokio::time::Instant::now() + timeout;
    let proxy = tokio::time::timeout_at(deadline, Screencast::new())
        .await
        .map_err(|_| PortalError::StartTimedOut)?
        .map_err(|e| map_error(&e))?;
    let session = tokio::time::timeout_at(deadline, proxy.create_session(Default::default()))
        .await
        .map_err(|_| PortalError::StartTimedOut)?
        .map_err(|e| map_error(&e))?;
    let rest = tokio::time::timeout_at(deadline, async {
        proxy
            .select_sources(
                &session,
                SelectSourcesOptions::default()
                    .set_sources(SourceType::MONITOR)
                    .set_multiple(true)
                    .set_cursor_mode(CursorMode::HIDDEN)
                    .set_persist_mode(PersistMode::ExplicitlyRevoked)
                    .set_restore_token(restore_token),
            )
            .await?;
        let response = proxy
            .start(&session, None, Default::default())
            .await?
            .response()?;
        let fd = proxy
            .open_pipe_wire_remote(&session, Default::default())
            .await?;
        Ok::<_, AshpdError>((response, fd))
    })
    .await;
    match rest {
        Ok(Ok((response, fd))) => Ok(Started {
            streams: stream_infos(&response),
            restore_token: response.restore_token().map(str::to_string),
            proxy,
            session,
            fd,
        }),
        Ok(Err(e)) => {
            close_quietly(&session).await;
            Err(map_error(&e))
        }
        Err(_elapsed) => {
            close_quietly(&session).await;
            Err(PortalError::StartTimedOut)
        }
    }
}

fn stream_infos(streams: &Streams) -> Vec<StreamInfo> {
    streams
        .streams()
        .iter()
        .map(|s| StreamInfo {
            node_id: s.pipe_wire_node_id(),
            position: s.position(),
            size: s.size(),
        })
        .collect()
}

async fn close_quietly(session: &Session<Screencast>) {
    if let Err(e) = session.close().await {
        tracing::debug!(kind = ?std::mem::discriminant(&e), "could not close the portal session");
    }
}

struct RealSession {
    rt: std::sync::Arc<Rt>,
    // Kept alive until the session is closed.
    #[allow(dead_code)]
    proxy: Screencast,
    session: Option<Session<Screencast>>,
    fd: Option<OwnedFd>,
    streams: Vec<StreamInfo>,
    restore_token: Option<String>,
}

impl PortalSession for RealSession {
    fn streams(&self) -> &[StreamInfo] {
        &self.streams
    }

    fn restore_token(&self) -> Option<&str> {
        self.restore_token.as_deref()
    }

    fn grab(&mut self, timeout: Duration) -> Result<Vec<RgbaImage>, PortalError> {
        let fd = self.fd.take().ok_or(PortalError::Failed)?;
        let nodes: Vec<u32> = self.streams.iter().map(|s| s.node_id).collect();
        // The PipeWire types are not `Send`, so they are created and dropped
        // on the helper thread; only the fd and the images cross over.
        off_runtime(move || pipewire_grab::grab_frames(fd, &nodes, timeout))?
    }

    fn close(mut self: Box<Self>) {
        let Some(session) = self.session.take() else {
            return;
        };
        let rt = std::sync::Arc::clone(&self.rt);
        let _ = off_runtime(|| {
            rt.get().block_on(async {
                let _ = tokio::time::timeout(
                    std::time::Duration::from_secs(2),
                    close_quietly(&session),
                )
                .await;
            });
        });
    }
}
