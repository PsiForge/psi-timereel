//! The Settings screen against a fake agent on a private session bus: Save
//! writes `config.toml` and calls `ReloadConfig`; confirming a lower
//! retention calls `RunRetention`. One test function, because it sets
//! `DBUS_SESSION_BUS_ADDRESS` for the whole process.

mod settings_support;

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};

use egui_kittest::kittest::Queryable;
use settings_support::*;
use timereel::agent_client::DbusAgent;
use timereel::settings::*;
use timereel_store::CaptureTime;
use zbus::{connection, interface};

struct Bus(Child);

impl Drop for Bus {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn start_bus() -> Bus {
    let mut child = Command::new("dbus-daemon")
        .args(["--session", "--nofork", "--print-address=1"])
        .stdout(Stdio::piped())
        .spawn()
        .expect("dbus-daemon is installed (package dbus)");
    let mut line = String::new();
    BufReader::new(child.stdout.take().expect("stdout"))
        .read_line(&mut line)
        .expect("bus address");
    std::env::set_var("DBUS_SESSION_BUS_ADDRESS", line.trim());
    Bus(child)
}

struct FakeAgent1 {
    calls: Arc<Mutex<Vec<String>>>,
}

#[interface(name = "org.timereel.Agent1")]
impl FakeAgent1 {
    fn reload_config(&self) -> String {
        self.calls
            .lock()
            .expect("calls")
            .push("ReloadConfig".into());
        String::new()
    }

    fn run_retention(&self) -> u32 {
        self.calls
            .lock()
            .expect("calls")
            .push("RunRetention".into());
        3
    }

    fn pause(&self) {
        self.calls.lock().expect("calls").push("Pause".into());
    }

    fn resume(&self) {
        self.calls.lock().expect("calls").push("Resume".into());
    }
}

const NOW: CaptureTime = CaptureTime(100 * DAY_MS);

fn harness(env: &TestEnv) -> SettingsHarness {
    settings_harness(env, Box::new(DbusAgent), NOW)
}

#[test]
fn save_and_confirmed_retention_reach_the_agent_over_d_bus() {
    let env = TestEnv::new();
    let _bus = start_bus();

    // No agent yet: the config is still saved, with the offline message.
    let mut h = harness(&env);
    set_field(&mut h, 0, "6");
    h.get_by_label(SAVE_LABEL).click();
    h.run_steps(2);
    assert_eq!(read_config().interval_minutes, 6);
    assert!(has_text(&h, SAVED_OFFLINE_MESSAGE));

    // Now the fake agent appears.
    let calls = Arc::new(Mutex::new(Vec::new()));
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .expect("runtime");
    let _connection = runtime
        .block_on(
            connection::Builder::session()
                .name("org.timereel.Agent1")
                .serve_at(
                    "/org/timereel/Agent1",
                    FakeAgent1 {
                        calls: calls.clone(),
                    },
                )
                .build(),
        )
        .expect("fake agent");

    set_field(&mut h, 0, "7");
    h.get_by_label(SAVE_LABEL).click();
    h.run_steps(2);
    let text = std::fs::read_to_string(env.config_file()).expect("config.toml");
    assert!(text.contains("interval_minutes = 7"), "{text}");
    assert_eq!(*calls.lock().expect("calls"), ["ReloadConfig"]);
    assert!(has_text(&h, SAVED_MESSAGE));

    // Lowering the retention: nothing happens until the user confirms.
    for days in [5, 15, 20, 40] {
        env.add_capture(CaptureTime(NOW.0 - days * DAY_MS));
    }
    calls.lock().expect("calls").clear();
    set_field(&mut h, 4, "10");
    h.get_by_label(SAVE_LABEL).click();
    h.run_steps(2);
    assert!(has_text(&h, "will delete 3 captures"));
    assert!(calls.lock().expect("calls").is_empty());
    h.get_by_label(CONFIRM_DELETE_LABEL).click();
    h.run_steps(2);
    assert_eq!(read_config().retention_days, 10);
    assert_eq!(
        *calls.lock().expect("calls"),
        ["ReloadConfig", "RunRetention"]
    );
    assert!(has_text(&h, "3 captures were deleted"));

    // Pause goes through D-Bus too.
    calls.lock().expect("calls").clear();
    h.get_by_label(PAUSE_LABEL).click();
    h.run_steps(2);
    assert_eq!(*calls.lock().expect("calls"), ["Pause"]);
}
