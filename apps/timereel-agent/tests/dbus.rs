//! The agent's D-Bus API on a private session bus (`dbus-daemon --session`,
//! the same thing `dbus-run-session` starts), with fake sources and a fake
//! clock. The config and data directories come from environment variables,
//! so the tests run one at a time.
#![allow(clippy::unwrap_used)]

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use age::x25519::Identity;
use futures_util::StreamExt;
use timereel_agent::clock::FakeClock;
use timereel_agent::cycle::CycleRunner;
use timereel_agent::runtime::{AgentHandle, Core};
use timereel_agent::{logind, service};
use timereel_capture::fake::{FakeLock, FakeScreen, FakeWindow};
use timereel_store::{CaptureMeta, Store};
use tokio::task::{JoinHandle, LocalSet};
use zbus::{connection, interface, proxy, Connection};

const T0: i64 = 1_747_215_000_000;
const MIN: i64 = 60_000;
const DAY: i64 = 24 * 60 * MIN;

static ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// The client side of `org.timereel.Agent1`.
#[proxy(
    interface = "org.timereel.Agent1",
    default_service = "org.timereel.Agent1",
    default_path = "/org/timereel/Agent1"
)]
trait Agent1 {
    #[zbus(property)]
    fn state(&self) -> zbus::Result<String>;
    #[zbus(property)]
    fn reason(&self) -> zbus::Result<String>;
    #[zbus(property)]
    fn last_capture_time(&self) -> zbus::Result<i64>;
    #[zbus(property)]
    fn last_skip_reason(&self) -> zbus::Result<String>;
    fn pause(&self) -> zbus::Result<()>;
    fn resume(&self) -> zbus::Result<()>;
    fn reload_config(&self) -> zbus::Result<String>;
    fn run_retention(&self) -> zbus::Result<u32>;
    fn retry_permission(&self) -> zbus::Result<()>;
}

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

struct Env {
    _guard: tokio::sync::MutexGuard<'static, ()>,
    dir: tempfile::TempDir,
    key: String,
    bus: Bus,
}

impl Env {
    async fn new() -> Env {
        let guard = ENV_LOCK.lock().await;
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var(timereel_config::ENV_CONFIG_DIR, dir.path().join("config"));
        std::env::set_var(timereel_config::ENV_DATA_DIR, dir.path().join("data"));
        std::env::set_var(timereel_config::ENV_STATE_DIR, dir.path().join("state"));
        let key = Identity::generate().to_public().to_string();
        let saved = key.clone();
        timereel_config::update(|c| c.public_key = Some(saved)).unwrap();
        Env {
            _guard: guard,
            dir,
            key,
            bus: Bus::start(),
        }
    }

    fn store(&self) -> Store {
        Store::new(self.dir.path().join("data"))
    }

    fn put_capture(&self, taken_at_ms: i64) {
        let meta = CaptureMeta {
            taken_at_ms,
            tz_offset_min: 0,
            app_name: "App".into(),
            window_title: "Title".into(),
            width: 1,
            height: 1,
            format: "webp".into(),
        };
        self.store()
            .write(&self.key, &meta, b"thumb", b"image")
            .unwrap();
    }
}

/// One running agent with its own bus connection.
struct Agent {
    clock: Arc<FakeClock>,
    screen: Arc<FakeScreen>,
    handle: AgentHandle,
    task: JoinHandle<()>,
    conn: Connection,
}

async fn start_agent(env: &Env) -> Agent {
    let clock = Arc::new(FakeClock::new(T0));
    let screen = Arc::new(FakeScreen::single_monitor());
    let runner = CycleRunner::new(
        Box::new(Arc::clone(&screen)),
        Box::new(FakeWindow::known("Firefox", "Inbox")),
        Box::new(FakeLock::new(false)),
        Box::new(env.store()),
    );
    let (core, handle) = Core::new(clock.clone(), runner, env.store());
    let conn = env.bus.connect().await;
    service::serve(&conn, handle.clone()).await.unwrap();
    Agent {
        clock,
        screen,
        handle,
        task: tokio::task::spawn_local(core.run()),
        conn,
    }
}

impl Agent {
    /// Stops the agent and frees its bus name, like a service restart.
    async fn stop(self) {
        self.handle.shutdown();
        self.task.await.unwrap();
        self.conn.graceful_shutdown().await;
    }
}

async fn client(env: &Env) -> Agent1Proxy<'static> {
    Agent1Proxy::new(&env.bus.connect().await).await.unwrap()
}

/// Polls until the `State` property equals `want`, for at most `within`.
async fn wait_state(proxy: &Agent1Proxy<'_>, want: &str, within: Duration) -> Duration {
    let start = Instant::now();
    loop {
        if proxy.state().await.unwrap() == want {
            return start.elapsed();
        }
        assert!(
            start.elapsed() < within,
            "State is {:?}, wanted {want:?} within {within:?}",
            proxy.state().await.unwrap()
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// Runs a test body on a current-thread runtime with a `LocalSet` (the
/// capture sources are not `Send`).
fn run<F: std::future::Future<Output = ()>>(future: F) {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(LocalSet::new().run_until(future));
}

#[test]
fn pause_and_resume_change_the_state_within_a_second_and_survive_a_restart() {
    run(async {
        let env = Env::new().await;
        let agent = start_agent(&env).await;
        let proxy = client(&env).await;
        assert_eq!(proxy.state().await.unwrap(), "recording");
        let mut changes = proxy.receive_state_changed().await;

        proxy.pause().await.unwrap();
        let took = wait_state(&proxy, "paused", Duration::from_secs(1)).await;
        assert!(took < Duration::from_secs(1));
        // PropertiesChanged arrives for State.
        let changed = tokio::time::timeout(Duration::from_secs(1), changes.next())
            .await
            .expect("PropertiesChanged for State")
            .unwrap();
        assert_eq!(changed.get().await.unwrap(), "paused");
        assert!(timereel_config::load().unwrap().paused, "saved in config");
        let text = std::fs::read_to_string(timereel_config::config_file().unwrap()).unwrap();
        assert!(text.contains("paused = true"), "config.toml: {text}");

        // A restart of the agent keeps the paused state.
        agent.stop().await;
        let agent = start_agent(&env).await;
        let proxy = client(&env).await;
        assert_eq!(proxy.state().await.unwrap(), "paused");
        agent.clock.advance(5 * MIN);
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(agent.screen.calls(), 0, "no capture while paused");
        assert_eq!(proxy.last_skip_reason().await.unwrap(), "paused");

        proxy.resume().await.unwrap();
        wait_state(&proxy, "recording", Duration::from_secs(1)).await;
        assert!(!timereel_config::load().unwrap().paused);
        agent.stop().await;
    });
}

#[test]
fn a_capture_shows_in_last_capture_time() {
    run(async {
        let env = Env::new().await;
        let agent = start_agent(&env).await;
        let proxy = client(&env).await;
        assert_eq!(proxy.last_capture_time().await.unwrap(), 0);
        assert_eq!(proxy.reason().await.unwrap(), "");
        agent.clock.advance(5 * MIN);
        let start = Instant::now();
        while proxy.last_capture_time().await.unwrap() == 0 {
            assert!(start.elapsed() < Duration::from_secs(2), "no capture");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(proxy.last_capture_time().await.unwrap(), T0 + 5 * MIN);
        assert_eq!(env.store().list(..).unwrap().len(), 1);
        proxy.retry_permission().await.unwrap();
        agent.stop().await;
    });
}

#[test]
fn reload_config_reschedules_and_an_invalid_config_returns_error_text() {
    run(async {
        let env = Env::new().await;
        let agent = start_agent(&env).await;
        let proxy = client(&env).await;
        assert_eq!(agent.handle.status().next_tick_ms, T0 + 5 * MIN);

        timereel_config::update(|c| c.interval_minutes = 15).unwrap();
        assert_eq!(proxy.reload_config().await.unwrap(), "");
        assert_eq!(agent.handle.status().next_tick_ms, T0 + 15 * MIN);
        // The old time passes with no tick: the schedule really moved.
        agent.clock.advance(5 * MIN);
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(agent.screen.calls(), 0);
        assert_eq!(agent.clock.wakeups(), 0);
        agent.clock.advance(10 * MIN);
        let start = Instant::now();
        while agent.screen.calls() == 0 {
            assert!(start.elapsed() < Duration::from_secs(2), "no tick");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        std::fs::write(
            timereel_config::config_file().unwrap(),
            "interval_minutes = 0\n",
        )
        .unwrap();
        let before = agent.handle.status().next_tick_ms;
        let text = proxy.reload_config().await.unwrap();
        assert!(text.contains("interval_minutes"), "error text: {text:?}");
        assert_eq!(agent.handle.status().next_tick_ms, before);
        agent.stop().await;
    });
}

#[test]
fn run_retention_returns_the_number_deleted() {
    run(async {
        let env = Env::new().await;
        let agent = start_agent(&env).await;
        let proxy = client(&env).await;
        env.put_capture(T0 - 31 * DAY);
        env.put_capture(T0 - 32 * DAY);
        env.put_capture(T0 - 90 * DAY);
        env.put_capture(T0 - DAY);
        assert_eq!(proxy.run_retention().await.unwrap(), 3);
        assert_eq!(env.store().list(..).unwrap().len(), 1);
        assert_eq!(proxy.run_retention().await.unwrap(), 0);
        agent.stop().await;
    });
}

/// A fake logind that sends `PrepareForSleep` on request.
struct FakeLogind;

#[interface(name = "org.freedesktop.login1.Manager")]
impl FakeLogind {
    #[zbus(signal)]
    async fn prepare_for_sleep(
        emitter: &zbus::object_server::SignalEmitter<'_>,
        start: bool,
    ) -> zbus::Result<()>;
}

#[test]
fn prepare_for_sleep_false_calls_the_resume_callback() {
    run(async {
        let env = Env::new().await;
        let logind_conn = connection::Builder::address(env.bus.address.as_str())
            .name("org.freedesktop.login1")
            .serve_at("/org/freedesktop/login1", FakeLogind)
            .build()
            .await
            .unwrap();
        let agent_conn = env.bus.connect().await;
        let resumed = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&resumed);
        let watcher = tokio::task::spawn_local(async move {
            logind::watch_sleep(&agent_conn, move || {
                counter.fetch_add(1, Ordering::SeqCst);
            })
            .await
        });
        let iface = logind_conn
            .object_server()
            .interface::<_, FakeLogind>("/org/freedesktop/login1")
            .await
            .unwrap();
        // Send until the watcher has subscribed (it starts asynchronously).
        let start = Instant::now();
        while resumed.load(Ordering::SeqCst) == 0 {
            assert!(start.elapsed() < Duration::from_secs(3), "no callback");
            FakeLogind::prepare_for_sleep(iface.signal_emitter(), true)
                .await
                .unwrap();
            FakeLogind::prepare_for_sleep(iface.signal_emitter(), false)
                .await
                .unwrap();
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        // The "going to sleep" signals alone never count: one per pair.
        assert!(resumed.load(Ordering::SeqCst) >= 1);
        // Finish the task inside the runtime: dropping a connection needs one.
        watcher.abort();
        let _ = watcher.await;
        drop(iface);
        logind_conn.graceful_shutdown().await;
    });
}
