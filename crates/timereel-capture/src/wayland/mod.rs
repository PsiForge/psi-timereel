//! Screen capture on Wayland through the ScreenCast portal (ADR 0008).
//!
//! Every capture opens a short portal session, grabs one frame per monitor
//! stream and closes the session again. The portal and PipeWire are behind
//! the [`Portal`] and [`PortalSession`] traits, so [`WaylandScreen`] and its
//! token and permission logic are tested with fakes. The real implementation
//! is [`ScreencastPortal`] (ashpd + pipewire-rs).
//!
//! # Restore token and permission
//!
//! * The stored restore token (`portal-restore-token` in the state directory,
//!   mode 0600) is passed to the portal at every start. The portal gives a
//!   new single-use token at every start; it replaces the stored one.
//! * With no token, or one the portal rejects, the portal itself shows its
//!   permission dialog during the start. That is the one prompt. If the user
//!   declines (or does not answer in time) the capture fails with
//!   [`CaptureError::NoPermission`], which the agent shows as
//!   `needs_permission` with the reason 'Screen capture permission denied'.
//! * After a denial the portal is **not** asked again, and the capture keeps
//!   failing with `NoPermission`, until [`ScreenSource::retry_permission`]
//!   (the `RetryPermission` D-Bus method) is called.
//!
//! No frame data is written to disk: pixels go from the PipeWire buffer into
//! memory, and the only file this module writes is the token.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use timereel_image::{Frame, RgbaImage};

use crate::{CaptureError, ScreenSource};

pub mod fake;
pub mod frame;
mod pipewire_grab;
mod portal;
pub mod token;

pub use portal::ScreencastPortal;
pub use token::TokenStore;

/// Time one capture may take (session, frames and close) when no permission
/// dialog can be involved.
pub const CAPTURE_TIMEOUT: Duration = Duration::from_secs(10);
/// Time the portal start may take when there is no stored token: a person
/// has to answer the permission dialog.
pub const PROMPT_TIMEOUT: Duration = Duration::from_secs(120);
/// The first ScreenCast portal version that supports restore tokens.
pub const MIN_PORTAL_VERSION: u32 = 4;

/// Why a portal call failed. It carries no free text, so it is safe to log.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PortalError {
    /// The user declined the permission dialog.
    #[error("the user declined screen capture")]
    Denied,
    /// The start did not finish in time. With a permission dialog open this
    /// means that nobody answered it.
    #[error("the portal start timed out")]
    StartTimedOut,
    /// Any other failure of the portal or of PipeWire.
    #[error("the screen capture portal failed")]
    Failed,
}

/// One monitor stream of a started portal session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamInfo {
    /// The PipeWire node to connect to.
    pub node_id: u32,
    /// The position on the desktop, if the portal told it.
    pub position: Option<(i32, i32)>,
    /// The logical size on the desktop, if the portal told it.
    pub size: Option<(i32, i32)>,
}

/// A started ScreenCast session.
pub trait PortalSession {
    /// The monitor streams of the session.
    fn streams(&self) -> &[StreamInfo];

    /// The new restore token the portal gave at the start, if any.
    fn restore_token(&self) -> Option<&str>;

    /// Grabs one frame per stream, in the order of [`PortalSession::streams`],
    /// converted to RGBA. It fails when `timeout` runs out.
    fn grab(&mut self, timeout: Duration) -> Result<Vec<RgbaImage>, PortalError>;

    /// Closes the session. Errors are only logged: the frames are done.
    fn close(self: Box<Self>);
}

/// The ScreenCast portal.
pub trait Portal {
    /// The version of the ScreenCast portal interface.
    fn version(&self) -> Result<u32, PortalError>;

    /// Creates a session, selects monitors (several, cursor hidden,
    /// `persist_mode` 2, with `restore_token`) and starts it. The portal may
    /// show its permission dialog here. `timeout` bounds the whole start.
    fn start(
        &self,
        restore_token: Option<&str>,
        timeout: Duration,
    ) -> Result<Box<dyn PortalSession>, PortalError>;
}

/// A [`ScreenSource`] for Wayland sessions.
pub struct WaylandScreen<P> {
    portal: P,
    tokens: TokenStore,
    /// Set after a denial; cleared by `retry_permission`.
    denied: AtomicBool,
}

impl<P: Portal> WaylandScreen<P> {
    /// A source that uses `portal` and keeps the restore token in `tokens`.
    pub fn new(portal: P, tokens: TokenStore) -> Self {
        Self {
            portal,
            tokens,
            denied: AtomicBool::new(false),
        }
    }

    /// Whether a denial is remembered (no prompts until `retry_permission`).
    pub fn is_denied(&self) -> bool {
        self.denied.load(Ordering::SeqCst)
    }

    fn start_session(&self) -> Result<Box<dyn PortalSession>, CaptureError> {
        let version = self.portal.version().map_err(|e| {
            tracing::warn!(error = %e, "could not read the portal version");
            CaptureError::Portal
        })?;
        if version < MIN_PORTAL_VERSION {
            tracing::warn!(
                version,
                required = MIN_PORTAL_VERSION,
                "the ScreenCast portal is too old for restore tokens"
            );
            return Err(CaptureError::PortalTooOld);
        }
        let token = self.tokens.load();
        let timeout = if token.is_some() {
            CAPTURE_TIMEOUT
        } else {
            // No token: the portal has to ask, and a person has to answer.
            PROMPT_TIMEOUT
        };
        match self.portal.start(token.as_deref(), timeout) {
            Ok(session) => Ok(session),
            Err(PortalError::Denied | PortalError::StartTimedOut) => {
                // A start that takes longer than the capture timeout with a
                // stored token means the token was rejected and the dialog is
                // waiting. Either way the prompt has been shown once.
                tracing::warn!("screen capture permission was not given");
                self.denied.store(true, Ordering::SeqCst);
                Err(CaptureError::NoPermission)
            }
            Err(PortalError::Failed) => Err(CaptureError::Portal),
        }
    }
}

impl<P: Portal> ScreenSource for WaylandScreen<P> {
    fn capture(&self) -> Result<Vec<Frame>, CaptureError> {
        if self.is_denied() {
            return Err(CaptureError::NoPermission);
        }
        let mut session = self.start_session()?;
        if let Some(new_token) = session.restore_token() {
            if let Err(e) = self.tokens.save(new_token) {
                // Not fatal for this capture, but the next start will prompt.
                tracing::warn!(error = %e, "could not save the portal restore token");
            }
        }
        let streams = session.streams().to_vec();
        let grabbed = session.grab(CAPTURE_TIMEOUT);
        session.close();
        let images = grabbed.map_err(|e| {
            tracing::warn!(error = %e, "could not grab the frames");
            CaptureError::Portal
        })?;
        if images.is_empty() || images.len() != streams.len() {
            tracing::warn!("the portal gave no usable streams");
            return Err(CaptureError::Portal);
        }
        layout(&streams, images)
    }

    fn retry_permission(&self) {
        if self.denied.swap(false, Ordering::SeqCst) {
            tracing::info!("screen capture will ask for permission again");
        }
    }
}

/// Puts each stream's image at its position. The portal gives logical
/// positions and sizes; an image with another pixel width (HiDPI) is scaled to
/// the logical width so the layout is right. Without positions the streams
/// are placed left to right.
fn layout(streams: &[StreamInfo], images: Vec<RgbaImage>) -> Result<Vec<Frame>, CaptureError> {
    let mut frames = Vec::with_capacity(images.len());
    let mut next_x = 0i32;
    for (info, image) in streams.iter().zip(images) {
        let image = match info.size {
            Some((w, _)) if w > 0 && w as u32 != image.width() => {
                timereel_image::scale_to_width(&image, w as u32)
                    .map_err(|_| CaptureError::Portal)?
            }
            _ => image,
        };
        let (x, y) = info.position.unwrap_or((next_x, 0));
        next_x = x.saturating_add(i32::try_from(image.width()).unwrap_or(i32::MAX));
        frames.push(Frame::new(x, y, image));
    }
    Ok(frames)
}

#[cfg(test)]
mod tests;
