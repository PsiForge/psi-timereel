//! The agent's main loop: scheduling, live config, pause and resume, and
//! retention (architecture sections 2.2 and 3).
//!
//! One task ([`Core::run`]) owns the [`CycleRunner`] and all scheduling state.
//! The D-Bus methods and the logind watcher talk to it through an
//! [`AgentHandle`], so there is no shared mutable state and no locking. The
//! loop sleeps until the earlier of the next tick and the next retention run
//! and does no work in between.
//!
//! Wall-clock schedule: the next tick is the last tick plus the interval. If
//! the machine was suspended (or the clock jumped) and that time is already
//! past, the tick happens at once and the schedule restarts from then, so
//! missed ticks are not made up.

use std::sync::Arc;

use timereel_config::Config;
use timereel_store::{retention, CaptureTime, Store};
use tokio::sync::{mpsc, oneshot, watch};

use crate::clock::Clock;
use crate::cycle::{AgentState, CycleRunner, Now};

/// Milliseconds in one minute.
const MS_PER_MINUTE: i64 = 60_000;
/// How often retention runs.
pub const RETENTION_PERIOD_MS: i64 = 24 * 60 * 60 * 1000;
/// `Reason` shown when the config file cannot be used at all.
const CONFIG_INVALID_REASON: &str = "Config file is invalid";

/// What the agent publishes. The first four fields are the D-Bus properties.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    /// The `State` property.
    pub state: AgentState,
    /// The `Reason` property.
    pub reason: String,
    /// The `LastCaptureTime` property: Unix ms, or 0.
    pub last_capture_ms: i64,
    /// The `LastSkipReason` property: empty if there was no skip yet.
    pub last_skip: String,
    /// Unix ms of the next scheduled tick (not on D-Bus; tests and logs).
    pub next_tick_ms: i64,
}

/// A request to the main loop.
#[derive(Debug)]
enum Command {
    SetPaused(bool, oneshot::Sender<Result<(), String>>),
    Reload(oneshot::Sender<String>),
    RunRetention(oneshot::Sender<Result<u32, String>>),
    Wake,
    Shutdown,
}

/// A cheap handle to talk to the running [`Core`].
#[derive(Debug, Clone)]
pub struct AgentHandle {
    tx: mpsc::UnboundedSender<Command>,
    status: watch::Receiver<Status>,
}

/// The agent has stopped, so a request could not be delivered.
const STOPPED: &str = "The agent is shutting down";

impl AgentHandle {
    /// The current status.
    pub fn status(&self) -> Status {
        self.status.borrow().clone()
    }

    /// A receiver that is notified at every status change.
    pub fn subscribe(&self) -> watch::Receiver<Status> {
        self.status.clone()
    }

    async fn request<T>(
        &self,
        make: impl FnOnce(oneshot::Sender<T>) -> Command,
    ) -> Result<T, String> {
        let (tx, rx) = oneshot::channel();
        self.tx.send(make(tx)).map_err(|_| STOPPED.to_string())?;
        rx.await.map_err(|_| STOPPED.to_string())
    }

    /// Saves `paused` to the config and applies it at once.
    pub async fn set_paused(&self, paused: bool) -> Result<(), String> {
        self.request(|r| Command::SetPaused(paused, r)).await?
    }

    /// Re-reads and validates the config and reschedules. Returns the error
    /// text if the config is invalid, and an empty string if it is fine.
    pub async fn reload_config(&self) -> Result<String, String> {
        self.request(Command::Reload).await
    }

    /// Runs retention now and returns the number of captures deleted.
    pub async fn run_retention(&self) -> Result<u32, String> {
        self.request(Command::RunRetention).await?
    }

    /// Tells the loop that the machine woke up from suspend, so it checks the
    /// wall clock again.
    pub fn notify_resumed(&self) {
        // If the loop is gone there is nothing left to wake.
        let _ = self.tx.send(Command::Wake);
    }

    /// Asks the loop to stop.
    pub fn shutdown(&self) {
        let _ = self.tx.send(Command::Shutdown);
    }
}

/// The main loop and its state.
pub struct Core {
    clock: Arc<dyn Clock>,
    runner: CycleRunner,
    store: Store,
    /// The last config that was read successfully.
    config: Option<Config>,
    next_tick: i64,
    next_retention: i64,
    status: watch::Sender<Status>,
    rx: mpsc::UnboundedReceiver<Command>,
}

impl Core {
    /// Sets up the loop: removes leftover partial files, runs retention once
    /// and schedules the first tick one interval from now. The initial state
    /// follows the config (`paused`, `not_set_up`, otherwise `recording`).
    pub fn new(clock: Arc<dyn Clock>, runner: CycleRunner, store: Store) -> (Core, AgentHandle) {
        let (tx, rx) = mpsc::unbounded_channel();
        let now = clock.now();
        let config = match timereel_config::load() {
            Ok(c) => Some(c),
            Err(_) => {
                tracing::warn!("config file is invalid at start");
                None
            }
        };
        let interval_ms = interval_ms(config.as_ref());
        let (status, status_rx) = watch::channel(Status {
            state: AgentState::Recording,
            reason: String::new(),
            last_capture_ms: 0,
            last_skip: String::new(),
            next_tick_ms: now.unix_ms + interval_ms,
        });
        let mut core = Core {
            clock,
            runner,
            store,
            config,
            next_tick: now.unix_ms + interval_ms,
            next_retention: now.unix_ms + RETENTION_PERIOD_MS,
            status,
            rx,
        };
        match core.store.cleanup_partials() {
            Ok(n) => tracing::info!(removed = n, "partial files cleaned up"),
            Err(e) => tracing::warn!(error = %e, "could not clean up partial files"),
        }
        if let Some(c) = core.config.clone() {
            core.runner.sync_with_config(&c);
            core.retention(now, c.retention_days);
        }
        core.publish();
        let handle = AgentHandle {
            tx,
            status: status_rx,
        };
        (core, handle)
    }

    /// Runs until [`AgentHandle::shutdown`] is called or every handle is
    /// dropped.
    pub async fn run(mut self) {
        loop {
            let deadline = self.next_tick.min(self.next_retention);
            let clock = Arc::clone(&self.clock);
            tokio::select! {
                () = clock.sleep_until(deadline) => self.on_wake(),
                cmd = self.rx.recv() => match cmd {
                    None | Some(Command::Shutdown) => break,
                    Some(cmd) => self.handle(cmd),
                },
            }
        }
        tracing::info!("agent loop stopped");
    }

    fn on_wake(&mut self) {
        let now = self.clock.now();
        if now.unix_ms >= self.next_tick {
            self.tick(now);
        }
        if now.unix_ms >= self.next_retention {
            let days = self.config.as_ref().map(|c| c.retention_days);
            if let Some(days) = days {
                self.retention(now, days);
            }
            self.next_retention = next_after(self.next_retention, RETENTION_PERIOD_MS, now);
        }
        self.publish();
    }

    /// One scheduled tick: re-read the config, run the cycle, schedule the next.
    fn tick(&mut self, now: Now) {
        let scheduled = self.next_tick;
        match timereel_config::load() {
            Ok(c) => self.config = Some(c),
            Err(_) => tracing::warn!("config file is invalid, using the last valid config"),
        }
        let interval = interval_ms(self.config.as_ref());
        self.next_tick = next_after(scheduled, interval, now);
        if let Some(config) = self.config.clone() {
            self.runner.run_once(&config, now);
        }
    }

    fn retention(&mut self, now: Now, days: u32) -> u32 {
        let report = retention::run(&self.store, CaptureTime(now.unix_ms), days);
        tracing::info!(
            deleted = report.deleted,
            failed = report.failed,
            "retention finished"
        );
        u32::try_from(report.deleted).unwrap_or(u32::MAX)
    }

    fn handle(&mut self, cmd: Command) {
        let now = self.clock.now();
        match cmd {
            Command::SetPaused(paused, reply) => {
                let result = self.set_paused(paused, now);
                // The caller may have gone away; the change is saved anyway.
                let _ = reply.send(result);
            }
            Command::Reload(reply) => {
                let text = self.reload(now);
                let _ = reply.send(text);
            }
            Command::RunRetention(reply) => {
                let result = match self.config.as_ref().map(|c| c.retention_days) {
                    Some(days) => Ok(self.retention(now, days)),
                    None => Err(CONFIG_INVALID_REASON.to_string()),
                };
                let _ = reply.send(result);
            }
            Command::Wake => tracing::info!("woke up from suspend, checking the schedule"),
            Command::Shutdown => {}
        }
        self.publish();
    }

    /// Pause and resume. Pausing takes effect at once: the state changes now
    /// and the tick that is already scheduled finds `paused` in the config and
    /// does not capture. Resuming schedules the next tick one interval away.
    fn set_paused(&mut self, paused: bool, now: Now) -> Result<(), String> {
        let config = timereel_config::update(|c| c.paused = paused).map_err(|e| {
            tracing::warn!("could not save the paused setting");
            e.to_string()
        })?;
        self.runner.sync_with_config(&config);
        self.next_tick = now.unix_ms + interval_ms(Some(&config));
        self.config = Some(config);
        tracing::info!(paused, "pause setting changed");
        Ok(())
    }

    /// `ReloadConfig`: returns the error text, or an empty string.
    fn reload(&mut self, now: Now) -> String {
        match timereel_config::load() {
            Ok(config) => {
                // Next tick = last tick + the new interval (at once if that is
                // already past). The last tick is the current schedule minus
                // the old interval.
                let last = self.next_tick - interval_ms(self.config.as_ref());
                self.next_tick = (last + interval_ms(Some(&config))).max(now.unix_ms);
                self.runner.sync_with_config(&config);
                self.config = Some(config);
                tracing::info!("config reloaded");
                String::new()
            }
            Err(e) => {
                tracing::warn!("reload refused: config is invalid");
                e.to_string()
            }
        }
    }

    fn publish(&self) {
        let (state, reason) = if self.config.is_none() {
            (AgentState::Error, CONFIG_INVALID_REASON)
        } else {
            (self.runner.state(), self.runner.reason())
        };
        let status = Status {
            state,
            reason: reason.to_string(),
            last_capture_ms: self.runner.last_capture_ms(),
            last_skip: self
                .runner
                .last_skip()
                .map(|s| s.as_str().to_string())
                .unwrap_or_default(),
            next_tick_ms: self.next_tick,
        };
        self.status.send_if_modified(|old| {
            if *old == status {
                false
            } else {
                *old = status;
                true
            }
        });
    }
}

/// The tick interval in ms for `config`; the default interval if there is none.
fn interval_ms(config: Option<&Config>) -> i64 {
    let minutes = config.map_or_else(
        || Config::default().interval_minutes,
        |c| c.interval_minutes,
    );
    i64::from(minutes) * MS_PER_MINUTE
}

/// The time after `last` plus `period`. If that is not in the future (the
/// machine slept, or the clock jumped) the schedule restarts from `now`.
fn next_after(last: i64, period: i64, now: Now) -> i64 {
    let next = last + period;
    if next > now.unix_ms {
        next
    } else {
        now.unix_ms + period
    }
}

#[cfg(test)]
mod tests;
