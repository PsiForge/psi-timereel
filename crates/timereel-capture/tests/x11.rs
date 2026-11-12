#![allow(clippy::unwrap_used, clippy::expect_used)]

//! Integration tests of the X11 sources against private Xvfb servers. Each
//! test starts its own server (the screen size must be 2560x1080), so they do
//! not depend on how `xvfb-run` was called.

use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use timereel_capture::{
    ActiveWindow, ActiveWindowSource, CaptureError, ScreenSource, X11Screen, X11Window,
};
use x11rb::connection::Connection;
use x11rb::protocol::randr::{ConnectionExt as _, MonitorInfo};
use x11rb::protocol::xproto::{
    AtomEnum, ConnectionExt as _, CreateGCAux, CreateWindowAux, PropMode, Rectangle, WindowClass,
};
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;
use x11rb::COPY_DEPTH_FROM_PARENT;

static NEXT_DISPLAY: AtomicU32 = AtomicU32::new(0);

struct Xvfb {
    child: Child,
    display: String,
}

impl Xvfb {
    /// Starts a server on a free display number.
    fn start(screen: &str) -> Self {
        for _ in 0..50 {
            let n = 300 + (std::process::id() % 400) + NEXT_DISPLAY.fetch_add(1, Ordering::SeqCst);
            if let Some(x) = Self::try_start(screen, n) {
                return x;
            }
        }
        panic!("could not start Xvfb (is the xvfb package installed?)");
    }

    /// Starts a server on display `n`; `None` if it exits (number taken).
    fn try_start(screen: &str, n: u32) -> Option<Self> {
        let display = format!(":{n}");
        let mut child = Command::new("Xvfb")
            .args([
                &display,
                "-screen",
                "0",
                screen,
                "-nolisten",
                "tcp",
                "-noreset",
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if child.try_wait().ok()?.is_some() {
                return None;
            }
            if RustConnection::connect(Some(&display)).is_ok() {
                return Some(Self { child, display });
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let _ = child.kill();
        None
    }

    fn connect(&self) -> (RustConnection, usize) {
        RustConnection::connect(Some(&self.display)).unwrap()
    }

    fn stop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for Xvfb {
    fn drop(&mut self) {
        self.stop();
    }
}

fn fill(conn: &RustConnection, root: u32, colour: u32, rect: (i16, i16, u16, u16)) {
    let gc = conn.generate_id().unwrap();
    conn.create_gc(gc, root, &CreateGCAux::new().foreground(colour))
        .unwrap();
    conn.poly_fill_rectangle(
        root,
        gc,
        &[Rectangle {
            x: rect.0,
            y: rect.1,
            width: rect.2,
            height: rect.3,
        }],
    )
    .unwrap();
    conn.free_gc(gc).unwrap();
}

/// Defines two 1280x1080 monitors, as with
/// `xrandr --setmonitor left 1280/338x1080/285+0+0 <output>` and
/// `xrandr --setmonitor right 1280/338x1080/285+1280+0 none`.
fn define_two_monitors(conn: &RustConnection, root: u32) {
    let outputs = conn
        .randr_get_screen_resources_current(root)
        .unwrap()
        .reply()
        .unwrap()
        .outputs;
    let atom = |name: &[u8]| conn.intern_atom(false, name).unwrap().reply().unwrap().atom;
    let monitor = |name, x, outputs| MonitorInfo {
        name,
        primary: x == 0,
        automatic: false,
        x,
        y: 0,
        width: 1280,
        height: 1080,
        width_in_millimeters: 338,
        height_in_millimeters: 285,
        outputs,
    };
    conn.randr_set_monitor(root, monitor(atom(b"left"), 0, outputs))
        .unwrap()
        .check()
        .unwrap();
    conn.randr_set_monitor(root, monitor(atom(b"right"), 1280, vec![]))
        .unwrap()
        .check()
        .unwrap();
    conn.sync().unwrap();
}

#[test]
fn two_virtual_monitors_are_captured_scaled_and_placed() {
    let xvfb = Xvfb::start("2560x1080x24");
    let (conn, num) = xvfb.connect();
    let root = conn.setup().roots[num].root;
    define_two_monitors(&conn, root);
    let monitors = conn
        .randr_get_monitors(root, true)
        .unwrap()
        .reply()
        .unwrap()
        .monitors;
    assert_eq!(monitors.len(), 2, "the test needs exactly two monitors");

    fill(&conn, root, 0xc80a0a, (0, 0, 1280, 1080));
    fill(&conn, root, 0x0a0ac8, (1280, 0, 1280, 1080));
    fill(&conn, root, 0x10c010, (100, 100, 200, 200));
    fill(&conn, root, 0xe0e000, (2280, 500, 200, 200));
    conn.sync().unwrap();

    let screen = X11Screen::with_display(Some(xvfb.display.clone()));
    screen.set_target_width(1280);
    let frames = screen.capture().unwrap();
    assert_eq!(frames.len(), 2);

    let image = timereel_image::stitch(&frames, 1280).unwrap();
    assert_eq!(image.width(), 1280);
    assert_eq!(image.height(), 540);
    let at = |x, y| image.pixel(x, y).unwrap();
    // Left monitor red, right monitor blue.
    assert_eq!(at(10, 10), [0xc8, 0x0a, 0x0a, 255]);
    assert_eq!(at(10, 530), [0xc8, 0x0a, 0x0a, 255]);
    assert_eq!(at(1270, 10), [0x0a, 0x0a, 0xc8, 255]);
    assert_eq!(at(1270, 530), [0x0a, 0x0a, 0xc8, 255]);
    // Source (100..300, 100..300) lands at (50..150, 50..150).
    assert_eq!(at(100, 100), [0x10, 0xc0, 0x10, 255]);
    assert_eq!(at(40, 100), [0xc8, 0x0a, 0x0a, 255]);
    assert_eq!(at(160, 100), [0xc8, 0x0a, 0x0a, 255]);
    // Source (2280..2480, 500..700) lands at (1140..1240, 250..350).
    assert_eq!(at(1190, 300), [0xe0, 0xe0, 0x00, 255]);
    assert_eq!(at(1130, 300), [0x0a, 0x0a, 0xc8, 255]);
    assert_eq!(at(1250, 300), [0x0a, 0x0a, 0xc8, 255]);
    // The two monitors meet in the middle.
    assert_eq!(at(639, 270), [0xc8, 0x0a, 0x0a, 255]);
    assert_eq!(at(640, 270), [0x0a, 0x0a, 0xc8, 255]);
}

#[test]
fn the_whole_screen_is_used_when_randr_lists_no_monitor() {
    let xvfb = Xvfb::start("1000x400x24");
    let (conn, num) = xvfb.connect();
    let root = conn.setup().roots[num].root;
    fill(&conn, root, 0x336699, (0, 0, 1000, 400));
    conn.sync().unwrap();

    let screen = X11Screen::with_display(Some(xvfb.display.clone()));
    screen.set_target_width(500);
    let frames = screen.capture().unwrap();
    let image = timereel_image::stitch(&frames, 500).unwrap();
    assert_eq!((image.width(), image.height()), (500, 200));
    assert_eq!(image.pixel(250, 100).unwrap(), [0x33, 0x66, 0x99, 255]);
}

#[test]
fn a_lost_connection_is_no_display_and_the_next_call_reconnects() {
    let mut xvfb = Xvfb::start("640x480x24");
    let display = xvfb.display.clone();
    let screen = X11Screen::with_display(Some(display.clone()));
    let window = X11Window::with_display(Some(display.clone()));
    assert!(screen.capture().is_ok());
    assert_eq!(window.active_window(), ActiveWindow::NoWindow);

    xvfb.stop();
    assert_eq!(screen.capture().err(), Some(CaptureError::NoDisplay));
    assert_eq!(screen.capture().err(), Some(CaptureError::NoDisplay));
    assert_eq!(window.active_window(), ActiveWindow::Unknown);

    let n: u32 = display.trim_start_matches(':').parse().unwrap();
    let _again = Xvfb::try_start("640x480x24", n).expect("restart on the same display");
    assert!(screen.capture().is_ok(), "reconnected at the next call");
    assert_eq!(window.active_window(), ActiveWindow::NoWindow);
}

#[test]
fn no_server_is_no_display() {
    let screen = X11Screen::with_display(Some(":997".to_string()));
    assert_eq!(screen.capture().err(), Some(CaptureError::NoDisplay));
    let window = X11Window::with_display(Some(":997".to_string()));
    assert_eq!(window.active_window(), ActiveWindow::Unknown);
}

/// What a window manager would do: windows with properties, and the
/// `_NET_ACTIVE_WINDOW` property on the root window.
struct Wm<'a> {
    conn: &'a RustConnection,
    root: u32,
    active: u32,
    net_wm_name: u32,
    utf8: u32,
}

impl<'a> Wm<'a> {
    fn new(conn: &'a RustConnection, root: u32) -> Self {
        let atom = |name: &[u8]| conn.intern_atom(false, name).unwrap().reply().unwrap().atom;
        Self {
            conn,
            root,
            active: atom(b"_NET_ACTIVE_WINDOW"),
            net_wm_name: atom(b"_NET_WM_NAME"),
            utf8: atom(b"UTF8_STRING"),
        }
    }

    fn new_window(&self) -> u32 {
        let id = self.conn.generate_id().unwrap();
        self.conn
            .create_window(
                COPY_DEPTH_FROM_PARENT,
                id,
                self.root,
                0,
                0,
                50,
                50,
                0,
                WindowClass::INPUT_OUTPUT,
                0,
                &CreateWindowAux::new(),
            )
            .unwrap();
        id
    }

    fn set_active(&self, id: u32) {
        self.conn
            .change_property32(
                PropMode::REPLACE,
                self.root,
                self.active,
                AtomEnum::WINDOW,
                &[id],
            )
            .unwrap();
        self.conn.sync().unwrap();
    }

    fn set_class(&self, win: u32, value: &[u8]) {
        self.conn
            .change_property8(
                PropMode::REPLACE,
                win,
                AtomEnum::WM_CLASS,
                AtomEnum::STRING,
                value,
            )
            .unwrap();
    }

    fn set_title(&self, win: u32, title: &str) {
        self.conn
            .change_property8(
                PropMode::REPLACE,
                win,
                self.net_wm_name,
                self.utf8,
                title.as_bytes(),
            )
            .unwrap();
    }
}

#[test]
fn the_active_window_is_read_from_ewmh_properties() {
    let xvfb = Xvfb::start("640x480x24");
    let (conn, num) = xvfb.connect();
    let root = conn.setup().roots[num].root;
    let wm = Wm::new(&conn, root);
    let source = X11Window::with_display(Some(xvfb.display.clone()));

    // Nothing set: no window.
    assert_eq!(source.active_window(), ActiveWindow::NoWindow);

    // A window with a known class and title.
    let win = wm.new_window();
    wm.set_class(win, b"myinst\0MyClass\0");
    wm.set_title(win, "Report — Entwurf ü");
    wm.set_active(win);
    assert_eq!(
        source.active_window(),
        ActiveWindow::Known {
            app: "MyClass".into(),
            title: "Report — Entwurf ü".into()
        }
    );

    // The title changes while the window stays active.
    wm.set_title(win, "Second title");
    conn.sync().unwrap();
    assert_eq!(
        source.active_window(),
        ActiveWindow::Known {
            app: "MyClass".into(),
            title: "Second title".into()
        }
    );

    // Empty class: the instance is the app. No _NET_WM_NAME: WM_NAME is the title.
    let legacy = wm.new_window();
    wm.set_class(legacy, b"xlegacy\0\0");
    conn.change_property8(
        PropMode::REPLACE,
        legacy,
        AtomEnum::WM_NAME,
        AtomEnum::STRING,
        b"Old title \xe4",
    )
    .unwrap();
    wm.set_active(legacy);
    assert_eq!(
        source.active_window(),
        ActiveWindow::Known {
            app: "xlegacy".into(),
            title: "Old title ä".into()
        }
    );

    // A window with a class but no title at all: known, empty title.
    let untitled = wm.new_window();
    wm.set_class(untitled, b"i\0Untitled\0");
    wm.set_active(untitled);
    assert_eq!(
        source.active_window(),
        ActiveWindow::Known {
            app: "Untitled".into(),
            title: String::new()
        }
    );

    // A window without WM_CLASS: the app cannot be told.
    let anonymous = wm.new_window();
    wm.set_active(anonymous);
    assert_eq!(source.active_window(), ActiveWindow::Unknown);

    // Window id 0: no window.
    wm.set_active(0);
    assert_eq!(source.active_window(), ActiveWindow::NoWindow);

    // A window that does not exist: property error, so unknown.
    wm.set_active(0x00ff_fff0);
    assert_eq!(source.active_window(), ActiveWindow::Unknown);

    // The property deleted again: no window.
    conn.delete_property(root, wm.active).unwrap();
    conn.sync().unwrap();
    assert_eq!(source.active_window(), ActiveWindow::NoWindow);
}
