//! The tray against a fake agent and a fake viewer on a private session bus
//! (`dbus-daemon --session`, the same thing `dbus-run-session` starts).
//!
//! The library tests drive `follow`, `Actions` and the tray model directly.
//! The binary tests start the real `timereel-tray` process and click its
//! menu through the `com.canonical.dbusmenu` interface that ksni serves.
#![allow(clippy::unwrap_used)]

use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use timereel_tray::bus::{follow, Actions, AGENT_NAME, AGENT_PATH, VIEWER_NAME, VIEWER_PATH};
use timereel_tray::model::{view, AgentSnapshot, IconKind, State, TrayView};
use timereel_tray::tray::TimeReelTray;
use tokio::sync::{watch, Notify};
use zbus::object_server::InterfaceRef;
use zbus::{connection, interface, proxy, Connection, OwnedVariant};

/// The calls a fake service has received, in order.
type Calls = Arc<Mutex<Vec<String>>>;

/// A private bus daemon, killed when dropped.
struct Bus {
    child: Child,
    address: String,
}

impl Bus {
    fn start() -> Bus {
        let mut child = Command::new("dbus-daemon")
            .args(["--session", "--nofork", "--print-address=1"])
            .stdout(Stdio::piped())
            .spawn()
            .expect("dbus-daemon is installed (package dbus)");
        let mut line = String::new();
        BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut line)
            .unwrap();
        Bus {
            child,
            address: line.trim().to_string(),
        }
    }

    async fn connect(&self) -> Connection {
        connection::Builder::address(self.address.as_str())
            .build()
            .await
            .unwrap()
    }
}

impl Drop for Bus {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

struct FakeAgent {
    state: String,
    reason: String,
    calls: Calls,
}

#[interface(name = "org.timereel.Agent1")]
impl FakeAgent {
    #[zbus(property)]
    async fn state(&self) -> String {
        self.state.clone()
    }

    #[zbus(property)]
    async fn reason(&self) -> String {
        self.reason.clone()
    }

    async fn pause(&self) {
        self.calls.lock().unwrap().push("Pause".into());
    }

    async fn resume(&self) {
        self.calls.lock().unwrap().push("Resume".into());
    }

    async fn retry_permission(&self) {
        self.calls.lock().unwrap().push("RetryPermission".into());
    }

    async fn reload_config(&self) -> String {
        self.calls.lock().unwrap().push("ReloadConfig".into());
        String::new()
    }

    async fn run_retention(&self) -> u32 {
        self.calls.lock().unwrap().push("RunRetention".into());
        0
    }
}

struct FakeViewer {
    calls: Calls,
}

#[interface(name = "org.timereel.Viewer1")]
impl FakeViewer {
    async fn activate(&self) {
        self.calls.lock().unwrap().push("Activate".into());
    }

    async fn quit(&self) {
        self.calls.lock().unwrap().push("Quit".into());
    }
}

/// A fake agent on its own connection.
struct Agent {
    conn: Connection,
    iface: InterfaceRef<FakeAgent>,
    calls: Calls,
}

async fn start_agent(bus: &Bus, state: &str, reason: &str) -> Agent {
    let calls = Calls::default();
    let conn = bus.connect().await;
    conn.object_server()
        .at(
            AGENT_PATH,
            FakeAgent {
                state: state.into(),
                reason: reason.into(),
                calls: Arc::clone(&calls),
            },
        )
        .await
        .unwrap();
    conn.request_name(AGENT_NAME).await.unwrap();
    let iface = conn.object_server().interface(AGENT_PATH).await.unwrap();
    Agent { conn, iface, calls }
}

impl Agent {
    async fn set(&self, state: &str, reason: &str) {
        let mut agent = self.iface.get_mut().await;
        agent.state = state.into();
        agent.reason = reason.into();
        agent
            .state_changed(self.iface.signal_emitter())
            .await
            .unwrap();
        agent
            .reason_changed(self.iface.signal_emitter())
            .await
            .unwrap();
    }

    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }

    /// Leaves the bus, like a stopped service.
    async fn leave(self) {
        // The interface reference holds a clone of the connection.
        drop(self.iface);
        self.conn.graceful_shutdown().await;
    }
}

async fn start_viewer(bus: &Bus) -> (Connection, Calls) {
    let calls = Calls::default();
    let conn = bus.connect().await;
    conn.object_server()
        .at(
            VIEWER_PATH,
            FakeViewer {
                calls: Arc::clone(&calls),
            },
        )
        .await
        .unwrap();
    conn.request_name(VIEWER_NAME).await.unwrap();
    (conn, calls)
}

/// Starts `follow` and returns the receiver of its snapshots.
async fn start_follower(bus: &Bus) -> watch::Receiver<AgentSnapshot> {
    let (tx, rx) = watch::channel(AgentSnapshot::NotRunning);
    let conn = bus.connect().await;
    tokio::spawn(follow(conn, tx));
    rx
}

/// Waits until the view of the latest snapshot satisfies `ok`, and returns
/// how long that took.
async fn wait_view(
    rx: &mut watch::Receiver<AgentSnapshot>,
    what: &str,
    ok: impl Fn(&TrayView) -> bool,
) -> Duration {
    let start = Instant::now();
    loop {
        let current = view(&rx.borrow_and_update());
        if ok(&current) {
            return start.elapsed();
        }
        let left = Duration::from_secs(5).saturating_sub(start.elapsed());
        assert!(
            !left.is_zero() && tokio::time::timeout(left, rx.changed()).await.is_ok(),
            "gave up waiting for: {what}; the view is {current:?}"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_state_change_reaches_the_tray_model_within_2_seconds() {
    let bus = Bus::start();
    let agent = start_agent(&bus, "recording", "").await;
    let mut rx = start_follower(&bus).await;
    wait_view(&mut rx, "recording icon", |v| v.icon == IconKind::Recording).await;

    agent.set("paused", "").await;
    let took = wait_view(&mut rx, "paused icon", |v| {
        v.icon == IconKind::Paused && v.menu[0].label == "Resume"
    })
    .await;
    assert!(took < Duration::from_secs(2), "took {took:?}");

    agent.set("error", "Disk full").await;
    let took = wait_view(&mut rx, "error with reason", |v| {
        v.icon == IconKind::Error && v.tooltip_text == "Disk full"
    })
    .await;
    assert!(took < Duration::from_secs(2), "took {took:?}");

    agent
        .set("needs_permission", "Screen capture permission denied")
        .await;
    wait_view(&mut rx, "grant entry", |v| {
        v.icon == IconKind::Error && v.menu.iter().any(|e| e.label == "Grant capture permission")
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_agent_leaving_the_bus_shows_agent_not_running() {
    let bus = Bus::start();
    let agent = start_agent(&bus, "recording", "").await;
    let mut rx = start_follower(&bus).await;
    wait_view(&mut rx, "recording icon", |v| v.icon == IconKind::Recording).await;

    agent.leave().await;
    let took = wait_view(&mut rx, "agent not running", |v| {
        v.icon == IconKind::Error && v.tooltip_text == "Agent not running"
    })
    .await;
    assert!(took < Duration::from_secs(2), "took {took:?}");

    // It comes back when the agent is back.
    let _agent = start_agent(&bus, "paused", "").await;
    wait_view(&mut rx, "paused again", |v| v.icon == IconKind::Paused).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_tray_starts_with_no_agent_on_the_bus() {
    let bus = Bus::start();
    let mut rx = start_follower(&bus).await;
    // The first snapshot is NotRunning, so this is the initial state, and it
    // must stay so.
    wait_view(&mut rx, "agent not running", |v| {
        v.tooltip_text == "Agent not running"
    })
    .await;
    let agent = start_agent(&bus, "recording", "").await;
    wait_view(&mut rx, "recording", |v| v.icon == IconKind::Recording).await;
    drop(agent);
}

/// Finds the menu entry with `label` and runs its action, like a click.
fn click(tray: &mut TimeReelTray, label: &str) {
    use ksni::menu::MenuItem;
    let mut action = None;
    for item in ksni::Tray::menu(tray) {
        if let MenuItem::Standard(item) = item {
            if item.label == label {
                action = Some(item.activate);
            }
        }
    }
    (action.unwrap_or_else(|| panic!("no menu entry {label:?}")))(tray);
}

async fn wait_calls(agent: &Agent, want: &[&str]) {
    let start = Instant::now();
    while agent.calls() != want {
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "agent calls are {:?}, wanted {want:?}",
            agent.calls()
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_pause_menu_entry_calls_pause_on_the_agent() {
    let bus = Bus::start();
    let agent = start_agent(&bus, "recording", "").await;
    let quit = Arc::new(Notify::new());
    let mut tray = TimeReelTray::new(Actions::new(bus.connect().await), Arc::clone(&quit));
    tray.set_snapshot(&AgentSnapshot::Running {
        state: State::Recording,
        reason: String::new(),
    });
    click(&mut tray, "Pause");
    wait_calls(&agent, &["Pause"]).await;

    tray.set_snapshot(&AgentSnapshot::Running {
        state: State::Paused,
        reason: String::new(),
    });
    click(&mut tray, "Resume");
    wait_calls(&agent, &["Pause", "Resume"]).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_grant_entry_calls_retry_permission() {
    let bus = Bus::start();
    let agent = start_agent(&bus, "needs_permission", "denied").await;
    let mut tray = TimeReelTray::new(Actions::new(bus.connect().await), Arc::new(Notify::new()));
    tray.set_snapshot(&AgentSnapshot::Running {
        state: State::NeedsPermission,
        reason: "denied".into(),
    });
    click(&mut tray, "Grant capture permission");
    wait_calls(&agent, &["RetryPermission"]).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn open_activates_a_running_viewer() {
    let bus = Bus::start();
    let (_viewer, viewer_calls) = start_viewer(&bus).await;
    let actions = Actions::new(bus.connect().await);
    actions.open_viewer().await.unwrap();
    assert_eq!(*viewer_calls.lock().unwrap(), ["Activate"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn quit_asks_the_viewer_and_never_calls_the_agent() {
    let bus = Bus::start();
    let agent = start_agent(&bus, "recording", "").await;
    let (_viewer, viewer_calls) = start_viewer(&bus).await;
    let quit = Arc::new(Notify::new());
    let mut tray = TimeReelTray::new(Actions::new(bus.connect().await), Arc::clone(&quit));
    click(&mut tray, "Quit");
    tokio::time::timeout(Duration::from_secs(5), quit.notified())
        .await
        .expect("the tray asks to exit");
    assert_eq!(*viewer_calls.lock().unwrap(), ["Quit"]);
    assert!(agent.calls().is_empty(), "{:?}", agent.calls());
}

// ---- The real binary, clicked through dbusmenu ----

#[proxy(interface = "com.canonical.dbusmenu", default_path = "/MenuBar")]
trait DbusMenu {
    fn get_layout(
        &self,
        parent_id: i32,
        recursion_depth: i32,
        property_names: &[&str],
    ) -> zbus::Result<(u32, (i32, HashMap<String, OwnedVariant>, Vec<OwnedVariant>))>;

    fn event(
        &self,
        id: i32,
        event_id: &str,
        data: &zbus::Variant<'_>,
        timestamp: u32,
    ) -> zbus::Result<()>;
}

/// The running `timereel-tray` process, killed when dropped.
struct TrayProcess {
    child: Child,
}

impl Drop for TrayProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl TrayProcess {
    fn start(bus: &Bus) -> TrayProcess {
        let child = Command::new(env!("CARGO_BIN_EXE_timereel-tray"))
            .env("DBUS_SESSION_BUS_ADDRESS", &bus.address)
            .spawn()
            .unwrap();
        TrayProcess { child }
    }

    fn item_name(&self) -> String {
        format!("org.kde.StatusNotifierItem-{}-1", self.child.id())
    }

    async fn menu<'a>(&self, conn: &'a Connection) -> DbusMenuProxy<'a> {
        let name = self.item_name();
        let start = Instant::now();
        loop {
            let proxy = DbusMenuProxy::builder(conn)
                .destination(name.clone())
                .build()
                .await
                .unwrap();
            if proxy.get_layout(0, -1, &[]).await.is_ok() {
                return proxy;
            }
            assert!(
                start.elapsed() < Duration::from_secs(10),
                "tray did not start"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
}

/// The (id, label) pairs of the menu.
async fn entries(menu: &DbusMenuProxy<'_>) -> Vec<(i32, String)> {
    let (_, (_, _, children)) = menu.get_layout(0, -1, &[]).await.unwrap();
    children
        .into_iter()
        .filter_map(|child| {
            let (id, props, _): (i32, HashMap<String, OwnedVariant>, Vec<OwnedVariant>) =
                child.try_into().ok()?;
            let label = props.get("label").and_then(|v| <&str>::try_from(v).ok())?;
            Some((id, label.to_string()))
        })
        .collect()
}

async fn click_in_menu(menu: &DbusMenuProxy<'_>, label: &str) {
    let start = Instant::now();
    loop {
        if let Some((id, _)) = entries(menu).await.into_iter().find(|(_, l)| l == label) {
            menu.event(id, "clicked", &zbus::Variant::I32(0), 0)
                .await
                .unwrap();
            return;
        }
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "no menu entry {label:?} in {:?}",
            entries(menu).await
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

fn wait_exit(process: &mut TrayProcess) -> bool {
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(5) {
        if process.child.try_wait().unwrap().is_some() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    false
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_process_pause_entry_calls_pause_and_the_label_follows_the_state() {
    let bus = Bus::start();
    let agent = start_agent(&bus, "recording", "").await;
    let tray = TrayProcess::start(&bus);
    let conn = bus.connect().await;
    let menu = tray.menu(&conn).await;

    click_in_menu(&menu, "Pause").await;
    wait_calls(&agent, &["Pause"]).await;

    agent.set("paused", "").await;
    click_in_menu(&menu, "Resume").await;
    wait_calls(&agent, &["Pause", "Resume"]).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_process_quit_entry_exits_without_touching_the_agent() {
    let bus = Bus::start();
    let agent = start_agent(&bus, "recording", "").await;
    let (_viewer, viewer_calls) = start_viewer(&bus).await;
    let mut tray = TrayProcess::start(&bus);
    let conn = bus.connect().await;
    let menu = tray.menu(&conn).await;

    click_in_menu(&menu, "Quit").await;
    assert!(wait_exit(&mut tray), "the tray did not exit after Quit");
    assert!(tray.child.try_wait().unwrap().unwrap().success());
    assert!(agent.calls().is_empty(), "{:?}", agent.calls());
    assert_eq!(*viewer_calls.lock().unwrap(), ["Quit"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_process_quits_even_without_an_agent_or_viewer() {
    let bus = Bus::start();
    let mut tray = TrayProcess::start(&bus);
    let conn = bus.connect().await;
    let menu = tray.menu(&conn).await;
    click_in_menu(&menu, "Quit").await;
    assert!(wait_exit(&mut tray), "the tray did not exit after Quit");
}
