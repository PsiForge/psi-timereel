#![allow(clippy::unwrap_used)]

//! Unit tests of the main loop with a fake clock and fake sources. The config
//! and data directories come from environment variables, so the tests run one
//! at a time.

use std::sync::Arc;
use std::time::Duration;

use age::x25519::Identity;
use timereel_capture::fake::{FakeLock, FakeScreen, FakeWindow};
use timereel_store::{CaptureMeta, CaptureTime, Store};
use tokio::task::JoinHandle;

use super::*;
use crate::clock::FakeClock;

const T0: i64 = 1_747_215_000_000;
const MIN: i64 = 60_000;
const DAY: i64 = 24 * 60 * MIN;

static ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

struct Env {
    _guard: tokio::sync::MutexGuard<'static, ()>,
    dir: tempfile::TempDir,
    key: String,
}

impl Env {
    /// Points the config and data directories at a fresh temp dir and saves a
    /// config with a public key (and `paused` as given).
    async fn new(with_key: bool) -> Env {
        let guard = ENV_LOCK.lock().await;
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var(timereel_config::ENV_CONFIG_DIR, dir.path().join("config"));
        std::env::set_var(timereel_config::ENV_DATA_DIR, dir.path().join("data"));
        std::env::set_var(timereel_config::ENV_STATE_DIR, dir.path().join("state"));
        let key = Identity::generate().to_public().to_string();
        let saved = key.clone();
        timereel_config::update(|c| {
            c.public_key = with_key.then_some(saved);
        })
        .unwrap();
        Env {
            _guard: guard,
            dir,
            key,
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

/// Runs `future` on a current-thread runtime with a `LocalSet`: the capture
/// sources are not `Send`, so the loop runs as a local task.
fn run<F: std::future::Future<Output = ()>>(future: F) {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let local = tokio::task::LocalSet::new();
    rt.block_on(local.run_until(future));
}

struct Running {
    clock: Arc<FakeClock>,
    screen: Arc<FakeScreen>,
    handle: AgentHandle,
    task: JoinHandle<()>,
}

fn start(env: &Env) -> Running {
    let clock = Arc::new(FakeClock::new(T0));
    let screen = Arc::new(FakeScreen::single_monitor());
    let runner = CycleRunner::new(
        Box::new(Arc::clone(&screen)),
        Box::new(FakeWindow::known("Firefox", "Inbox")),
        Box::new(FakeLock::new(false)),
        Box::new(env.store()),
    );
    let (core, handle) = Core::new(clock.clone(), runner, env.store());
    Running {
        clock,
        screen,
        handle,
        task: tokio::task::spawn_local(core.run()),
    }
}

impl Running {
    /// Lets the loop run until `cond` holds (or fails after 2 s).
    async fn until(&self, what: &str, cond: impl Fn(&Running) -> bool) {
        for _ in 0..400 {
            if cond(self) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!("timed out waiting for: {what}");
    }

    /// Gives the loop time to (wrongly) do something.
    async fn settle(&self) {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    async fn stop(self) {
        self.handle.shutdown();
        self.task.await.unwrap();
    }
}

#[test]
fn no_public_key_is_not_set_up_and_nothing_is_captured() {
    run(async {
        let env = Env::new(false).await;
        let agent = start(&env);
        assert_eq!(agent.handle.status().state, AgentState::NotSetUp);
        assert_eq!(
            agent.handle.status().reason,
            "No public key: run set-up in TimeReel"
        );
        // Run through two ticks.
        agent.clock.advance(5 * MIN);
        agent.until("first tick", |a| a.clock.wakeups() == 1).await;
        agent.clock.advance(5 * MIN);
        agent.until("second tick", |a| a.clock.wakeups() == 2).await;
        agent.settle().await;
        assert_eq!(agent.screen.calls(), 0, "no capture without a key");
        assert_eq!(agent.handle.status().state, AgentState::NotSetUp);
        assert!(env.store().list(..).unwrap().is_empty());
        agent.stop().await;
    });
}

#[test]
fn no_ticks_happen_between_scheduled_times() {
    run(async {
        let env = Env::new(true).await;
        let agent = start(&env);
        assert_eq!(agent.handle.status().state, AgentState::Recording);
        assert_eq!(agent.handle.status().next_tick_ms, T0 + 5 * MIN);
        for _ in 0..4 {
            agent.clock.advance(MIN);
            agent.settle().await;
            assert_eq!(agent.clock.wakeups(), 0, "woke before the tick");
            assert_eq!(agent.screen.calls(), 0);
        }
        agent.clock.advance(MIN);
        agent.until("tick", |a| a.screen.calls() == 1).await;
        assert_eq!(agent.clock.wakeups(), 1);
        assert_eq!(agent.handle.status().next_tick_ms, T0 + 10 * MIN);
        // And quiet again until the next one.
        agent.clock.advance(4 * MIN);
        agent.settle().await;
        assert_eq!((agent.clock.wakeups(), agent.screen.calls()), (1, 1));
        agent.stop().await;
    });
}

#[test]
fn a_long_jump_of_the_clock_gives_one_tick_not_a_burst() {
    run(async {
        let env = Env::new(true).await;
        let agent = start(&env);
        agent.clock.advance(3 * 60 * MIN); // like a suspend
        agent.handle.notify_resumed();
        agent.until("tick", |a| a.screen.calls() == 1).await;
        agent.settle().await;
        assert_eq!(agent.screen.calls(), 1);
        assert_eq!(
            agent.handle.status().next_tick_ms,
            T0 + 3 * 60 * MIN + 5 * MIN
        );
        agent.stop().await;
    });
}

#[test]
fn pause_cancels_the_pending_capture_and_resume_reschedules() {
    run(async {
        let env = Env::new(true).await;
        let agent = start(&env);
        agent.clock.advance(4 * MIN);
        agent.handle.set_paused(true).await.unwrap();
        assert_eq!(agent.handle.status().state, AgentState::Paused);
        assert!(timereel_config::load().unwrap().paused);
        // The tick that was pending at T0 + 5 min does not capture.
        agent.clock.advance(2 * MIN);
        agent.settle().await;
        assert_eq!(agent.screen.calls(), 0);

        agent.handle.set_paused(false).await.unwrap();
        assert_eq!(agent.handle.status().state, AgentState::Recording);
        assert!(!timereel_config::load().unwrap().paused);
        assert_eq!(agent.handle.status().next_tick_ms, T0 + 6 * MIN + 5 * MIN);
        agent.clock.advance(5 * MIN);
        agent.until("capture", |a| a.screen.calls() == 1).await;
        agent.stop().await;
    });
}

#[test]
fn retry_permission_tells_the_source_and_captures_at_once() {
    run(async {
        let env = Env::new(true).await;
        let agent = start(&env);
        assert_eq!(agent.screen.retries(), 0);
        // No time passes: the next tick is a full interval away.
        agent.handle.retry_permission().await.unwrap();
        assert_eq!(agent.screen.retries(), 1);
        agent.until("capture", |a| a.screen.calls() == 1).await;
        agent.stop().await;
    });
}

#[test]
fn the_config_is_read_again_at_every_tick() {
    run(async {
        let env = Env::new(true).await;
        let agent = start(&env);
        // Edited behind the agent's back, no ReloadConfig.
        timereel_config::update(|c| {
            c.paused = true;
            c.interval_minutes = 10;
        })
        .unwrap();
        agent.clock.advance(5 * MIN);
        agent.until("tick", |a| a.clock.wakeups() == 1).await;
        agent
            .until("paused", |a| a.handle.status().state == AgentState::Paused)
            .await;
        assert_eq!(agent.screen.calls(), 0);
        assert_eq!(agent.handle.status().last_skip, "paused");
        assert_eq!(agent.handle.status().next_tick_ms, T0 + 15 * MIN);
        agent.stop().await;
    });
}

#[test]
fn reload_config_reschedules_and_refuses_an_invalid_config() {
    run(async {
        let env = Env::new(true).await;
        let agent = start(&env);
        agent.clock.advance(2 * MIN);
        timereel_config::update(|c| c.interval_minutes = 10).unwrap();
        assert_eq!(agent.handle.reload_config().await.unwrap(), "");
        // Last tick was T0 (the start); the new interval is 10 min.
        assert_eq!(agent.handle.status().next_tick_ms, T0 + 10 * MIN);

        std::fs::write(
            timereel_config::config_file().unwrap(),
            "interval_minutes = 0\n",
        )
        .unwrap();
        let text = agent.handle.reload_config().await.unwrap();
        assert!(text.contains("interval_minutes"), "text: {text}");
        assert_eq!(agent.handle.status().next_tick_ms, T0 + 10 * MIN);
        agent.stop().await;
    });
}

#[test]
fn start_cleans_partial_files_and_runs_retention() {
    run(async {
        let env = Env::new(true).await;
        env.put_capture(T0 - 40 * DAY);
        env.put_capture(T0 - DAY);
        let old = env.store().path_of(CaptureTime(T0 - 40 * DAY));
        let partial = old.with_file_name(".20250101T000000000Z.partial");
        std::fs::write(&partial, b"half").unwrap();
        let agent = start(&env);
        assert!(!partial.exists(), "partial file was not cleaned up");
        assert!(!old.exists(), "old capture was not deleted at start");
        assert_eq!(env.store().list(..).unwrap().len(), 1);
        agent.stop().await;
    });
}

#[test]
fn retention_runs_every_24_hours() {
    run(async {
        let env = Env::new(true).await;
        let agent = start(&env);
        env.put_capture(T0 - 40 * DAY);
        let old = env.store().path_of(CaptureTime(T0 - 40 * DAY));
        agent.clock.advance(DAY - MIN);
        agent.settle().await;
        // Ticks happened, but retention is not due yet.
        assert!(old.exists());
        agent.clock.advance(MIN);
        agent.until("retention", |_| !old.exists()).await;
        agent.stop().await;
    });
}

#[test]
fn run_retention_returns_the_number_deleted() {
    run(async {
        let env = Env::new(true).await;
        let agent = start(&env);
        env.put_capture(T0 - 31 * DAY);
        env.put_capture(T0 - 35 * DAY);
        env.put_capture(T0 - DAY);
        assert_eq!(agent.handle.run_retention().await.unwrap(), 2);
        assert_eq!(agent.handle.run_retention().await.unwrap(), 0);
        agent.stop().await;
    });
}

#[test]
fn a_broken_config_at_start_is_an_error_state_and_nothing_is_captured() {
    run(async {
        let env = Env::new(true).await;
        std::fs::write(timereel_config::config_file().unwrap(), "not = [valid").unwrap();
        let agent = start(&env);
        assert_eq!(agent.handle.status().state, AgentState::Error);
        agent.clock.advance(5 * MIN);
        agent.until("tick", |a| a.clock.wakeups() == 1).await;
        agent.settle().await;
        assert_eq!(agent.screen.calls(), 0);
        // Fixed by the owner: the next reload brings the agent back.
        std::fs::write(
            timereel_config::config_file().unwrap(),
            format!("public_key = '{}'\n", env.key),
        )
        .unwrap();
        assert_eq!(agent.handle.reload_config().await.unwrap(), "");
        assert_eq!(agent.handle.status().state, AgentState::Recording);
        agent.stop().await;
    });
}
