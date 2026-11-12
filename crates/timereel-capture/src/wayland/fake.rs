//! A scriptable fake of the ScreenCast portal, for tests.
//!
//! It behaves like the real portal where it matters for the token logic: a
//! start with no token, or with a token that is not the one it handed out
//! last, shows a permission dialog (counted in [`FakePortal::prompts`]) that
//! the test script answers with grant or deny. A start with the right token
//! is silent and hands out a new token, which makes the old one invalid.

use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use timereel_image::RgbaImage;

use super::frame::{convert_to_rgba, PixelFormat};
use super::{Portal, PortalError, PortalSession, StreamInfo};

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// One fake monitor: raw PipeWire-style bytes and where it sits.
#[derive(Debug, Clone)]
pub struct FakeStream {
    /// What the portal reports for the stream.
    pub info: StreamInfo,
    /// Pixel layout of `data`.
    pub format: PixelFormat,
    /// Pixel width of `data`.
    pub width: u32,
    /// Pixel height of `data`.
    pub height: u32,
    /// Bytes per row of `data` (may be padded).
    pub stride: i64,
    /// The raw frame bytes.
    pub data: Vec<u8>,
}

impl FakeStream {
    /// A stream with a solid colour `rgb`, tightly packed, at `position`.
    pub fn solid(
        node_id: u32,
        position: (i32, i32),
        width: u32,
        height: u32,
        rgb: [u8; 3],
    ) -> Self {
        let px = [rgb[2], rgb[1], rgb[0], 0];
        Self {
            info: StreamInfo {
                node_id,
                position: Some(position),
                size: Some((width as i32, height as i32)),
            },
            format: PixelFormat::Bgrx,
            width,
            height,
            stride: i64::from(width) * 4,
            data: px.repeat((width * height) as usize),
        }
    }
}

/// The fake portal. Share it with `Arc` to look at it after a capture.
pub struct FakePortal {
    version: u32,
    streams: Vec<FakeStream>,
    grant: Mutex<bool>,
    valid_token: Mutex<Option<String>>,
    received: Mutex<Vec<Option<String>>>,
    prompts: AtomicUsize,
    starts: AtomicUsize,
    closed: AtomicUsize,
    counter: AtomicU32,
}

impl FakePortal {
    /// A portal of `version` with one 4x2 monitor. The user grants prompts.
    pub fn new(version: u32) -> Self {
        Self::with_streams(
            version,
            vec![FakeStream::solid(1, (0, 0), 4, 2, [10, 20, 30])],
        )
    }

    /// A portal with the given monitors.
    pub fn with_streams(version: u32, streams: Vec<FakeStream>) -> Self {
        Self {
            version,
            streams,
            grant: Mutex::new(true),
            valid_token: Mutex::new(None),
            received: Mutex::new(Vec::new()),
            prompts: AtomicUsize::new(0),
            starts: AtomicUsize::new(0),
            closed: AtomicUsize::new(0),
            counter: AtomicU32::new(0),
        }
    }

    /// Makes `token` the one the portal accepts without a dialog.
    pub fn accept_token(&self, token: &str) {
        *lock(&self.valid_token) = Some(token.to_string());
    }

    /// Sets whether the user grants (true) or declines (false) the dialog.
    pub fn set_grant(&self, grant: bool) {
        *lock(&self.grant) = grant;
    }

    /// How many permission dialogs were shown.
    pub fn prompts(&self) -> usize {
        self.prompts.load(Ordering::SeqCst)
    }

    /// How many times `start` was called.
    pub fn starts(&self) -> usize {
        self.starts.load(Ordering::SeqCst)
    }

    /// How many sessions were closed.
    pub fn closed(&self) -> usize {
        self.closed.load(Ordering::SeqCst)
    }

    /// The tokens passed to `start`, in order.
    pub fn received_tokens(&self) -> Vec<Option<String>> {
        lock(&self.received).clone()
    }

    /// The token the portal accepts now.
    pub fn current_token(&self) -> Option<String> {
        lock(&self.valid_token).clone()
    }
}

impl Portal for Arc<FakePortal> {
    fn version(&self) -> Result<u32, PortalError> {
        Ok(self.version)
    }

    fn start(
        &self,
        restore_token: Option<&str>,
        _timeout: Duration,
    ) -> Result<Box<dyn PortalSession>, PortalError> {
        self.starts.fetch_add(1, Ordering::SeqCst);
        lock(&self.received).push(restore_token.map(str::to_string));
        let silent = restore_token.is_some() && restore_token == lock(&self.valid_token).as_deref();
        if !silent {
            self.prompts.fetch_add(1, Ordering::SeqCst);
            if !*lock(&self.grant) {
                return Err(PortalError::Denied);
            }
        }
        let new_token = format!("token-{}", self.counter.fetch_add(1, Ordering::SeqCst) + 1);
        *lock(&self.valid_token) = Some(new_token.clone());
        Ok(Box::new(FakeSession {
            portal: Arc::clone(self),
            infos: self.streams.iter().map(|s| s.info).collect(),
            token: new_token,
        }))
    }
}

struct FakeSession {
    portal: Arc<FakePortal>,
    infos: Vec<StreamInfo>,
    token: String,
}

impl PortalSession for FakeSession {
    fn streams(&self) -> &[StreamInfo] {
        &self.infos
    }

    fn restore_token(&self) -> Option<&str> {
        Some(&self.token)
    }

    fn grab(&mut self, _timeout: Duration) -> Result<Vec<RgbaImage>, PortalError> {
        self.portal
            .streams
            .iter()
            .map(|s| {
                convert_to_rgba(s.format, s.width, s.height, s.stride, &s.data)
                    .map_err(|_| PortalError::Failed)
            })
            .collect()
    }

    fn close(self: Box<Self>) {
        self.portal.closed.fetch_add(1, Ordering::SeqCst);
    }
}
