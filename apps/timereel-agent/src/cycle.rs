//! One capture cycle and the agent state machine (architecture sections 2.2
//! and 10).
//!
//! [`CycleRunner::run_once`] does the work of one tick, in this order:
//!
//! 1. paused: skip with reason `paused`;
//! 2. locked: skip with reason `locked`;
//! 3. active window: an excluded app skips with `excluded`, an unknown app
//!    skips with `app-unknown` unless `capture_when_app_unknown` is set, and
//!    no focused window is captured with the app name `(none)`;
//! 4. capture all monitors;
//! 5. ask for the active window again: if an excluded app became active, the
//!    frame is thrown away (`excluded`);
//! 6. stitch, scale, thumbnail, encode, then seal and store.
//!
//! Logging uses `tracing` and only ever carries skip reasons and error kinds,
//! never window titles, app names, image data or keys.

use timereel_capture::{ActiveWindow, ActiveWindowSource, CaptureError, LockSource, ScreenSource};
use timereel_config::{Config, ImageFormat};
use timereel_crypto::ContainerError;
use timereel_image::Format;
use timereel_store::{CaptureMeta, CaptureTime, Store, StoreError};

/// App name stored when no window has the focus.
pub const NO_WINDOW_APP: &str = "(none)";

/// Linux errno for "No space left on device".
const ENOSPC: i32 = 28;

/// The time of a cycle: Unix time in ms (UTC) and the local UTC offset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Now {
    /// Milliseconds since the Unix epoch, UTC.
    pub unix_ms: i64,
    /// Offset of local time from UTC in minutes.
    pub tz_offset_min: i32,
}

/// The agent state shown to the tray and on D-Bus (`State` property).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentState {
    /// Captures are being taken.
    Recording,
    /// The user paused recording.
    Paused,
    /// The last capture failed.
    Error,
    /// Screen capture permission is missing.
    NeedsPermission,
    /// There is no public key yet.
    NotSetUp,
}

impl AgentState {
    /// The D-Bus string of the state.
    pub fn as_str(self) -> &'static str {
        match self {
            AgentState::Recording => "recording",
            AgentState::Paused => "paused",
            AgentState::Error => "error",
            AgentState::NeedsPermission => "needs_permission",
            AgentState::NotSetUp => "not_set_up",
        }
    }
}

/// Why a cycle did not capture. Never holds content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkipReason {
    /// Recording is paused.
    Paused,
    /// The screen is locked.
    Locked,
    /// An excluded app is active.
    Excluded,
    /// The active app is unknown and capturing it is not allowed.
    AppUnknown,
}

impl SkipReason {
    /// The `LastSkipReason` string: `paused`, `locked`, `excluded` or `app-unknown`.
    pub fn as_str(self) -> &'static str {
        match self {
            SkipReason::Paused => "paused",
            SkipReason::Locked => "locked",
            SkipReason::Excluded => "excluded",
            SkipReason::AppUnknown => "app-unknown",
        }
    }
}

/// What went wrong in a cycle (architecture section 10).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    /// No display found.
    NoDisplay,
    /// Screen capture permission denied.
    NoPermission,
    /// Screen capture portal error.
    Portal,
    /// The ScreenCast portal is older than version 4.
    PortalTooOld,
    /// The disk is full (ENOSPC).
    DiskFull,
    /// Encryption failed.
    Encryption,
    /// The capture could not be saved.
    Storage,
    /// There is no public key.
    NotSetUp,
}

impl ErrorKind {
    /// The kind name used in logs.
    pub fn as_str(self) -> &'static str {
        match self {
            ErrorKind::NoDisplay => "NoDisplay",
            ErrorKind::NoPermission => "NoPermission",
            ErrorKind::Portal => "Portal",
            ErrorKind::PortalTooOld => "PortalTooOld",
            ErrorKind::DiskFull => "DiskFull",
            ErrorKind::Encryption => "Encryption",
            ErrorKind::Storage => "Storage",
            ErrorKind::NotSetUp => "NotSetUp",
        }
    }

    /// The short reason shown to the user (`Reason` property).
    pub fn reason(self) -> &'static str {
        match self {
            ErrorKind::NoDisplay => "No display found",
            ErrorKind::NoPermission => "Screen capture permission denied",
            ErrorKind::Portal => "Screen capture portal error",
            ErrorKind::PortalTooOld => {
                "Screen capture portal is too old: version 4 or later is needed"
            }
            ErrorKind::DiskFull => "Disk full",
            ErrorKind::Encryption => "Encryption failed",
            ErrorKind::Storage => "Could not save the screenshot",
            ErrorKind::NotSetUp => "No public key: run set-up in TimeReel",
        }
    }

    /// The agent state this error leads to.
    pub fn state(self) -> AgentState {
        match self {
            ErrorKind::NoPermission => AgentState::NeedsPermission,
            ErrorKind::NotSetUp => AgentState::NotSetUp,
            _ => AgentState::Error,
        }
    }
}

impl From<CaptureError> for ErrorKind {
    fn from(e: CaptureError) -> Self {
        match e {
            CaptureError::NoDisplay => ErrorKind::NoDisplay,
            CaptureError::NoPermission => ErrorKind::NoPermission,
            CaptureError::Portal => ErrorKind::Portal,
            CaptureError::PortalTooOld => ErrorKind::PortalTooOld,
        }
    }
}

impl From<&StoreError> for ErrorKind {
    fn from(e: &StoreError) -> Self {
        match e {
            StoreError::Io { source, .. } if source.raw_os_error() == Some(ENOSPC) => {
                ErrorKind::DiskFull
            }
            StoreError::Container(ContainerError::Seal(_)) => ErrorKind::Encryption,
            _ => ErrorKind::Storage,
        }
    }
}

/// The result of one cycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// A capture was stored.
    Captured(CaptureTime),
    /// Nothing was captured, on purpose.
    Skipped(SkipReason),
    /// The cycle failed.
    Failed(ErrorKind),
}

/// Where finished captures go. [`Store`] is the real implementation; tests
/// use a fake to produce errors such as ENOSPC.
pub trait CaptureSink {
    /// Seals and stores one capture.
    fn write(
        &self,
        public_key: &str,
        meta: &CaptureMeta,
        thumb: &[u8],
        image: &[u8],
    ) -> Result<CaptureTime, StoreError>;
}

impl CaptureSink for Store {
    fn write(
        &self,
        public_key: &str,
        meta: &CaptureMeta,
        thumb: &[u8],
        image: &[u8],
    ) -> Result<CaptureTime, StoreError> {
        Store::write(self, public_key, meta, thumb, image)
    }
}

/// Whether `app` is on the excluded list: a case-insensitive match on the
/// whole name (no partial matches).
pub fn is_excluded(excluded_apps: &[String], app: &str) -> bool {
    let app = app.to_lowercase();
    excluded_apps.iter().any(|e| e.to_lowercase() == app)
}

/// Runs capture cycles and keeps the agent state.
pub struct CycleRunner {
    screen: Box<dyn ScreenSource>,
    window: Box<dyn ActiveWindowSource>,
    lock: Box<dyn LockSource>,
    sink: Box<dyn CaptureSink>,
    state: AgentState,
    reason: String,
    last_capture_ms: i64,
    last_skip: Option<SkipReason>,
}

impl CycleRunner {
    /// Creates a runner in the `recording` state.
    pub fn new(
        screen: Box<dyn ScreenSource>,
        window: Box<dyn ActiveWindowSource>,
        lock: Box<dyn LockSource>,
        sink: Box<dyn CaptureSink>,
    ) -> Self {
        Self {
            screen,
            window,
            lock,
            sink,
            state: AgentState::Recording,
            reason: String::new(),
            last_capture_ms: 0,
            last_skip: None,
        }
    }

    /// The current state.
    pub fn state(&self) -> AgentState {
        self.state
    }

    /// The short user-facing reason: empty unless the state is `error`,
    /// `needs_permission` or `not_set_up`.
    pub fn reason(&self) -> &str {
        &self.reason
    }

    /// Unix time in ms of the last stored capture, or 0.
    pub fn last_capture_ms(&self) -> i64 {
        self.last_capture_ms
    }

    /// The reason of the last skip, if any.
    pub fn last_skip(&self) -> Option<SkipReason> {
        self.last_skip
    }

    /// Runs one capture cycle with the config read for this tick.
    ///
    /// The state afterwards: `paused` after a paused skip; the error state of
    /// a failure; `recording` after a success. Other skips leave an error
    /// state as it is (only a success clears it) but end `paused`.
    pub fn run_once(&mut self, config: &Config, now: Now) -> Outcome {
        let outcome = self.cycle(config, now);
        match outcome {
            Outcome::Captured(time) => {
                self.last_capture_ms = time.0;
                self.set_state(AgentState::Recording, "");
                tracing::debug!("capture stored");
            }
            Outcome::Skipped(reason) => {
                self.last_skip = Some(reason);
                if reason == SkipReason::Paused {
                    self.set_state(AgentState::Paused, "");
                } else if self.state == AgentState::Paused {
                    self.set_state(AgentState::Recording, "");
                }
                tracing::info!(reason = reason.as_str(), "capture skipped");
            }
            Outcome::Failed(kind) => {
                self.set_state(kind.state(), kind.reason());
                tracing::warn!(kind = kind.as_str(), "capture failed");
            }
        }
        outcome
    }

    /// Brings the state in line with a config that was just read or saved,
    /// without waiting for the next tick: `paused` wins, then a missing public
    /// key gives `not_set_up`, and a state that no longer applies
    /// (`paused` or `not_set_up`) goes back to `recording`. An error state is
    /// kept; only a successful capture clears it.
    pub fn sync_with_config(&mut self, config: &Config) {
        if config.paused {
            self.set_state(AgentState::Paused, "");
        } else if config.public_key.is_none() {
            self.set_state(AgentState::NotSetUp, ErrorKind::NotSetUp.reason());
        } else if matches!(self.state, AgentState::Paused | AgentState::NotSetUp) {
            self.set_state(AgentState::Recording, "");
        }
    }

    /// Tells the screen source that the user asked for screen capture
    /// permission again (`RetryPermission`). The caller runs a cycle next.
    pub fn retry_permission(&mut self) {
        self.screen.retry_permission();
    }

    fn set_state(&mut self, state: AgentState, reason: &str) {
        self.state = state;
        self.reason.clear();
        self.reason.push_str(reason);
    }

    fn cycle(&mut self, config: &Config, now: Now) -> Outcome {
        if config.paused {
            return Outcome::Skipped(SkipReason::Paused);
        }
        if self.lock.is_locked() {
            return Outcome::Skipped(SkipReason::Locked);
        }
        let (app, title) = match self.window.active_window() {
            ActiveWindow::Known { app, title } => {
                if is_excluded(&config.excluded_apps, &app) {
                    return Outcome::Skipped(SkipReason::Excluded);
                }
                (app, title)
            }
            ActiveWindow::NoWindow => (NO_WINDOW_APP.to_string(), String::new()),
            ActiveWindow::Unknown => {
                if !config.capture_when_app_unknown {
                    return Outcome::Skipped(SkipReason::AppUnknown);
                }
                ("unknown".to_string(), "unknown".to_string())
            }
        };
        let Some(public_key) = config.public_key.as_deref() else {
            return Outcome::Failed(ErrorKind::NotSetUp);
        };

        self.screen.set_target_width(config.width_px);
        let frames = match self.screen.capture() {
            Ok(frames) => frames,
            Err(e) => return Outcome::Failed(e.into()),
        };

        if let ActiveWindow::Known { app: now_app, .. } = self.window.active_window() {
            if is_excluded(&config.excluded_apps, &now_app) {
                return Outcome::Skipped(SkipReason::Excluded);
            }
        }

        let meta = CaptureMeta {
            taken_at_ms: now.unix_ms,
            tz_offset_min: now.tz_offset_min,
            app_name: app,
            window_title: title,
            width: 0,
            height: 0,
            format: format_name(config.format).to_string(),
        };
        match encode_and_store(self.sink.as_ref(), public_key, config, &frames, meta) {
            Ok(time) => Outcome::Captured(time),
            Err(kind) => Outcome::Failed(kind),
        }
    }
}

fn format_name(format: ImageFormat) -> &'static str {
    match format {
        ImageFormat::Webp => "webp",
        ImageFormat::Jpeg => "jpeg",
    }
}

/// Image pipeline, then seal and store. Image errors count as `Storage`:
/// the screenshot could not be turned into a file.
fn encode_and_store(
    sink: &dyn CaptureSink,
    public_key: &str,
    config: &Config,
    frames: &[timereel_image::Frame],
    mut meta: CaptureMeta,
) -> Result<CaptureTime, ErrorKind> {
    let format = match config.format {
        ImageFormat::Webp => Format::Webp,
        ImageFormat::Jpeg => Format::Jpeg,
    };
    // The config is validated (quality 1..=100), so the conversion cannot fail.
    let quality = u8::try_from(config.quality).unwrap_or(timereel_image::DEFAULT_QUALITY);
    let image = timereel_image::stitch(frames, config.width_px).map_err(|_| ErrorKind::Storage)?;
    let thumb = timereel_image::thumbnail(&image, config.thumbnail_width_px)
        .map_err(|_| ErrorKind::Storage)?;
    meta.width = image.width();
    meta.height = image.height();
    let image_bytes =
        timereel_image::encode(&image, format, quality).map_err(|_| ErrorKind::Storage)?;
    let thumb_bytes =
        timereel_image::encode(&thumb, format, timereel_image::DEFAULT_THUMBNAIL_QUALITY)
            .map_err(|_| ErrorKind::Storage)?;
    sink.write(public_key, &meta, &thumb_bytes, &image_bytes)
        .map_err(|e| ErrorKind::from(&e))
}

#[cfg(test)]
mod tests;
