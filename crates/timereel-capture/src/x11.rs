//! X11 sources (ADR 0007): screens through RandR and `GetImage`, and the
//! active window through EWMH.
//!
//! Both sources connect lazily and keep one connection. If the connection is
//! lost they drop it and connect again at the next call, so a restarted X
//! server is picked up at the next tick. Log messages hold error kinds only,
//! never window titles or app names.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Mutex, MutexGuard};

use timereel_image::{Frame, DEFAULT_WIDTH_PX};
use x11rb::connection::Connection;
use x11rb::errors::{ConnectError, ConnectionError, ReplyError};
use x11rb::protocol::randr::ConnectionExt as _;
use x11rb::protocol::xproto::{
    Atom, AtomEnum, ConnectionExt as _, ImageFormat, ImageOrder, Window,
};
use x11rb::rust_connection::RustConnection;

use crate::band::{capture_scaled, BandError, BandSource, Rect};
use crate::{ActiveWindow, ActiveWindowSource, CaptureError, ScreenSource};

/// Request size for text properties, in 4-byte units.
const TEXT_LONGS: u32 = 1024;
/// Request size for `WM_CLASS`, in 4-byte units.
const CLASS_LONGS: u32 = 256;

/// Source of the display name: the `DISPLAY` variable, or a fixed name.
#[derive(Debug, Clone, Default)]
struct DisplayName(Option<String>);

struct Atoms {
    net_active_window: Atom,
    net_wm_name: Atom,
    utf8_string: Atom,
}

struct Conn {
    conn: RustConnection,
    root: Window,
    root_size: (u32, u32),
    atoms: Atoms,
    /// Whether the root window has the 32-bit BGRx layout we convert from.
    bgrx: bool,
}

/// A connection failure, either of the kind the caller must reconnect after
/// (`lost`) or one it can retry on the same connection.
#[derive(Debug, Clone, Copy)]
struct Failure {
    kind: &'static str,
    lost: bool,
}

impl From<ConnectionError> for Failure {
    fn from(_: ConnectionError) -> Self {
        Failure {
            kind: "connection",
            lost: true,
        }
    }
}

impl From<ReplyError> for Failure {
    fn from(e: ReplyError) -> Self {
        match e {
            ReplyError::ConnectionError(_) => Failure {
                kind: "connection",
                lost: true,
            },
            ReplyError::X11Error(_) => Failure {
                kind: "x11-error",
                lost: false,
            },
        }
    }
}

impl From<BandError> for Failure {
    fn from(e: BandError) -> Self {
        match e {
            BandError::Source("connection") => Failure {
                kind: "connection",
                lost: true,
            },
            BandError::Source(kind) => Failure { kind, lost: false },
            BandError::Layout => Failure {
                kind: "layout",
                lost: false,
            },
            BandError::BadData => Failure {
                kind: "pixel-data",
                lost: false,
            },
        }
    }
}

impl Conn {
    /// Connects and reads what later calls need. The error is a fixed kind.
    fn open(display: &DisplayName) -> Result<Self, &'static str> {
        let (conn, screen_num) =
            RustConnection::connect(display.0.as_deref()).map_err(|e| connect_kind(&e))?;
        let setup = conn.setup();
        let screen = &setup.roots[screen_num];
        let root = screen.root;
        let root_size = (
            u32::from(screen.width_in_pixels),
            u32::from(screen.height_in_pixels),
        );
        let bgrx = setup.image_byte_order == ImageOrder::LSB_FIRST
            && setup.pixmap_formats.iter().any(|f| {
                f.depth == screen.root_depth && f.bits_per_pixel == 32 && f.scanline_pad == 32
            });
        let atoms = Self::intern_atoms(&conn).map_err(|_| "atoms")?;
        Ok(Self {
            conn,
            root,
            root_size,
            atoms,
            bgrx,
        })
    }

    fn intern_atoms(conn: &RustConnection) -> Result<Atoms, ReplyError> {
        let active = conn.intern_atom(false, b"_NET_ACTIVE_WINDOW")?;
        let name = conn.intern_atom(false, b"_NET_WM_NAME")?;
        let utf8 = conn.intern_atom(false, b"UTF8_STRING")?;
        Ok(Atoms {
            net_active_window: active.reply()?.atom,
            net_wm_name: name.reply()?.atom,
            utf8_string: utf8.reply()?.atom,
        })
    }

    /// The monitor rectangles from RandR `GetMonitors`. Without RandR, or if
    /// it lists nothing, the whole root window counts as one monitor.
    fn monitors(&self) -> Result<Vec<Rect>, Failure> {
        let listed = match self.conn.randr_get_monitors(self.root, true) {
            Ok(cookie) => match cookie.reply() {
                Ok(reply) => Some(reply.monitors),
                Err(ReplyError::X11Error(_)) => None,
                Err(e) => return Err(e.into()),
            },
            Err(ConnectionError::UnsupportedExtension) => None,
            Err(e) => return Err(e.into()),
        };
        let rects: Vec<Rect> = listed
            .unwrap_or_default()
            .iter()
            .filter(|m| m.width > 0 && m.height > 0)
            .map(|m| Rect {
                x: i32::from(m.x),
                y: i32::from(m.y),
                width: u32::from(m.width),
                height: u32::from(m.height),
            })
            .collect();
        if rects.is_empty() {
            return Ok(vec![Rect {
                x: 0,
                y: 0,
                width: self.root_size.0,
                height: self.root_size.1,
            }]);
        }
        Ok(rects)
    }

    fn capture(&self, target_width: u32) -> Result<Vec<Frame>, Failure> {
        if !self.bgrx {
            return Err(Failure {
                kind: "pixel-format",
                lost: false,
            });
        }
        let monitors = self.monitors()?;
        let mut grabber = Grabber { conn: self };
        Ok(capture_scaled(&mut grabber, &monitors, target_width)?)
    }
}

/// Reads bands from the root window with `GetImage`.
struct Grabber<'a> {
    conn: &'a Conn,
}

impl BandSource for Grabber<'_> {
    fn read_band(
        &mut self,
        x: i32,
        y: i32,
        width: u32,
        rows: u32,
        consume: &mut dyn FnMut(&[u8]) -> Result<(), BandError>,
    ) -> Result<(), BandError> {
        let to_i16 = |v: i32| i16::try_from(v).map_err(|_| BandError::Layout);
        let to_u16 = |v: u32| u16::try_from(v).map_err(|_| BandError::Layout);
        let reply = self
            .conn
            .conn
            .get_image(
                ImageFormat::Z_PIXMAP,
                self.conn.root,
                to_i16(x)?,
                to_i16(y)?,
                to_u16(width)?,
                to_u16(rows)?,
                !0,
            )
            .map_err(|_| BandError::Source("connection"))?
            .reply()
            .map_err(|e| match e {
                ReplyError::ConnectionError(_) => BandError::Source("connection"),
                ReplyError::X11Error(_) => BandError::Source("x11-error"),
            })?;
        // `reply.data` is the only raw data held: one band.
        consume(&reply.data)
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    // The data is a connection that is dropped on any failure, so a poisoned
    // lock holds nothing that needs to be trusted.
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// A lazily opened, self-healing connection.
struct Link {
    display: DisplayName,
    conn: Mutex<Option<Conn>>,
}

impl Link {
    fn new(display: Option<String>) -> Self {
        Self {
            display: DisplayName(display),
            conn: Mutex::new(None),
        }
    }

    /// Runs `f` on the connection, opening it first if needed. A lost
    /// connection is dropped so the next call reconnects.
    fn with<T>(&self, f: impl FnOnce(&Conn) -> Result<T, Failure>) -> Result<T, Failure> {
        let mut guard = lock(&self.conn);
        if guard.is_none() {
            match Conn::open(&self.display) {
                Ok(c) => *guard = Some(c),
                Err(kind) => {
                    tracing::warn!(kind, "cannot connect to the X server");
                    return Err(Failure {
                        kind: "connect",
                        lost: true,
                    });
                }
            }
        }
        let result = match guard.as_ref() {
            Some(c) => f(c),
            None => {
                return Err(Failure {
                    kind: "connect",
                    lost: true,
                })
            }
        };
        if matches!(&result, Err(Failure { lost: true, .. })) {
            *guard = None;
        }
        result
    }
}

fn connect_kind(e: &ConnectError) -> &'static str {
    match e {
        ConnectError::UnknownError => "unknown",
        ConnectError::ParseError(_) => "bad-display-name",
        ConnectError::InsufficientMemory => "memory",
        ConnectError::DisplayParsingError(_) => "bad-display-name",
        ConnectError::InvalidScreen => "invalid-screen",
        ConnectError::IoError(_) => "io",
        ConnectError::ZeroIdMask => "zero-id-mask",
        ConnectError::SetupAuthenticate(_) | ConnectError::SetupFailed(_) => "setup",
        _ => "other",
    }
}

/// Captures all monitors of an X11 display.
///
/// Each monitor is read in bands of at most
/// [`MAX_BAND_ROWS`](crate::band::MAX_BAND_ROWS) rows and scaled as it
/// arrives, so the frames returned are already scaled to the target width
/// (see [`X11Screen::set_target_width`]) and the peak memory is about one
/// band. Any failure, including a lost connection, is
/// [`CaptureError::NoDisplay`]; the next call connects again.
pub struct X11Screen {
    link: Link,
    target_width: AtomicU32,
}

impl X11Screen {
    /// A source for the display named by `DISPLAY`.
    pub fn new() -> Self {
        Self::with_display(None)
    }

    /// A source for the given display name (for example `:0`), or for
    /// `DISPLAY` if `None`. It connects at the first capture.
    pub fn with_display(display: Option<String>) -> Self {
        Self {
            link: Link::new(display),
            target_width: AtomicU32::new(DEFAULT_WIDTH_PX),
        }
    }

    /// Sets the width the combined layout is scaled to (never upscaled).
    /// The agent calls this before each capture with the configured width.
    pub fn set_target_width(&self, width: u32) {
        self.target_width.store(width.max(1), Ordering::Relaxed);
    }
}

impl Default for X11Screen {
    fn default() -> Self {
        Self::new()
    }
}

impl ScreenSource for X11Screen {
    fn capture(&self) -> Result<Vec<Frame>, CaptureError> {
        let width = self.target_width.load(Ordering::Relaxed);
        self.link.with(|c| c.capture(width)).map_err(|f| {
            tracing::warn!(kind = f.kind, "X11 screen capture failed");
            CaptureError::NoDisplay
        })
    }

    fn set_target_width(&self, width: u32) {
        X11Screen::set_target_width(self, width);
    }
}

/// Reads the focused window of an X11 display from EWMH properties.
///
/// The app is the class part of `WM_CLASS` (the instance part if the class is
/// empty). The title is `_NET_WM_NAME`, or `WM_NAME` if that is missing.
/// `_NET_ACTIVE_WINDOW` unset or 0 is [`ActiveWindow::NoWindow`]; any
/// property error, or a window without a readable app, is
/// [`ActiveWindow::Unknown`].
pub struct X11Window {
    link: Link,
}

impl X11Window {
    /// A source for the display named by `DISPLAY`.
    pub fn new() -> Self {
        Self::with_display(None)
    }

    /// A source for the given display name, or `DISPLAY` if `None`. It
    /// connects at the first call.
    pub fn with_display(display: Option<String>) -> Self {
        Self {
            link: Link::new(display),
        }
    }
}

impl Default for X11Window {
    fn default() -> Self {
        Self::new()
    }
}

impl ActiveWindowSource for X11Window {
    fn active_window(&self) -> ActiveWindow {
        match self.link.with(read_active_window) {
            Ok(w) => w,
            Err(f) => {
                tracing::warn!(kind = f.kind, "cannot read the active window");
                ActiveWindow::Unknown
            }
        }
    }
}

fn read_active_window(c: &Conn) -> Result<ActiveWindow, Failure> {
    let reply = c
        .conn
        .get_property(
            false,
            c.root,
            c.atoms.net_active_window,
            AtomEnum::WINDOW,
            0,
            1,
        )?
        .reply()?;
    if reply.type_ == u32::from(AtomEnum::NONE) {
        return Ok(ActiveWindow::NoWindow);
    }
    let Some(id) = reply.value32().and_then(|mut v| v.next()) else {
        // Wrong format or no value: the property is unusable.
        return Ok(if reply.value_len == 0 {
            ActiveWindow::NoWindow
        } else {
            ActiveWindow::Unknown
        });
    };
    if id == 0 {
        return Ok(ActiveWindow::NoWindow);
    }

    let class = c
        .conn
        .get_property(
            false,
            id,
            AtomEnum::WM_CLASS,
            AtomEnum::STRING,
            0,
            CLASS_LONGS,
        )?
        .reply()?;
    let Some(app) = app_from_wm_class(&class.value) else {
        return Ok(ActiveWindow::Unknown);
    };

    let net_name = c
        .conn
        .get_property(
            false,
            id,
            c.atoms.net_wm_name,
            c.atoms.utf8_string,
            0,
            TEXT_LONGS,
        )?
        .reply()?;
    let mut title = decode_text(&net_name.value, net_name.type_);
    if title.is_empty() {
        let name = c
            .conn
            .get_property(false, id, AtomEnum::WM_NAME, AtomEnum::ANY, 0, TEXT_LONGS)?
            .reply()?;
        title = decode_text(&name.value, name.type_);
    }
    Ok(ActiveWindow::Known { app, title })
}

/// The app name from a `WM_CLASS` value (`instance\0class\0`): the class,
/// or the instance if the class is empty. `None` if both are empty.
fn app_from_wm_class(value: &[u8]) -> Option<String> {
    let mut parts = value.split(|&b| b == 0);
    let instance = parts.next().unwrap_or_default();
    let class = parts.next().unwrap_or_default();
    let chosen = if class.is_empty() { instance } else { class };
    if chosen.is_empty() {
        return None;
    }
    Some(chosen.iter().map(|&b| char::from(b)).collect())
}

/// Decodes a text property: UTF-8 for `UTF8_STRING`, Latin-1 for `STRING`
/// (the `WM_NAME` default), lossy UTF-8 for anything else. Stops at the
/// first NUL. A missing property (type `NONE`) gives an empty string.
fn decode_text(value: &[u8], type_: Atom) -> String {
    let end = value.iter().position(|&b| b == 0).unwrap_or(value.len());
    let value = &value[..end];
    if type_ == u32::from(AtomEnum::NONE) {
        String::new()
    } else if type_ == u32::from(AtomEnum::STRING) {
        value.iter().map(|&b| char::from(b)).collect()
    } else {
        String::from_utf8_lossy(value).into_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn class_wins_over_instance() {
        assert_eq!(
            app_from_wm_class(b"navigator\0Firefox\0").as_deref(),
            Some("Firefox")
        );
    }

    #[test]
    fn instance_is_the_fallback() {
        assert_eq!(app_from_wm_class(b"xterm\0\0").as_deref(), Some("xterm"));
        assert_eq!(app_from_wm_class(b"xterm").as_deref(), Some("xterm"));
    }

    #[test]
    fn empty_class_property_is_unknown() {
        assert_eq!(app_from_wm_class(b""), None);
        assert_eq!(app_from_wm_class(b"\0\0"), None);
    }

    #[test]
    fn text_decoding() {
        let utf8 = 999;
        assert_eq!(decode_text("Grüße".as_bytes(), utf8), "Grüße");
        assert_eq!(
            decode_text(b"Gr\xfc\xdfe", u32::from(AtomEnum::STRING)),
            "Grüße"
        );
        assert_eq!(decode_text(b"a\0b", utf8), "a");
        assert_eq!(decode_text(b"x", u32::from(AtomEnum::NONE)), "");
    }
}
