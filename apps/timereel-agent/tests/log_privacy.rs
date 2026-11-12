//! The log of a full run holds skip reasons and error kinds only: no window
//! title or app name. This is its own test binary because it installs a
//! global `tracing` subscriber (callsite caching makes scoped subscribers
//! unreliable when other tests run in parallel).
#![allow(clippy::unwrap_used)]

use std::io;
use std::sync::{Arc, Mutex};

use age::x25519::Identity;
use timereel_agent::cycle::{CaptureSink, CycleRunner, Now, Outcome};
use timereel_capture::fake::{FakeLock, FakeScreen, FakeWindow};
use timereel_capture::{ActiveWindow, CaptureError};
use timereel_config::Config;
use timereel_store::{CaptureMeta, CaptureTime, Store, StoreError};

const APP: &str = "SecretApp-Zq81";
const TITLE: &str = "SecretTitle-Xw42";
const NOW: Now = Now {
    unix_ms: 1_747_215_000_123,
    tz_offset_min: 0,
};

#[derive(Clone, Default)]
struct LogBuf(Arc<Mutex<Vec<u8>>>);

impl io::Write for LogBuf {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

struct DiskFullSink;

impl CaptureSink for DiskFullSink {
    fn write(
        &self,
        _: &str,
        _: &CaptureMeta,
        _: &[u8],
        _: &[u8],
    ) -> Result<CaptureTime, StoreError> {
        Err(StoreError::Io {
            action: "write",
            path: format!("/data/{APP}").into(),
            source: io::Error::from_raw_os_error(28),
        })
    }
}

fn runner(
    dir: &std::path::Path,
    window: FakeWindow,
    locked: bool,
    screen: FakeScreen,
) -> CycleRunner {
    CycleRunner::new(
        Box::new(screen),
        Box::new(window),
        Box::new(FakeLock::new(locked)),
        Box::new(Store::new(dir)),
    )
}

#[test]
fn log_of_a_full_run_has_no_app_or_title() {
    let buf = LogBuf::default();
    let writer = buf.clone();
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::TRACE)
        .with_ansi(false)
        .with_writer(move || writer.clone())
        .init();

    let dir = tempfile::tempdir().unwrap();
    let config = Config {
        public_key: Some(Identity::generate().to_public().to_string()),
        excluded_apps: vec![APP.to_lowercase()],
        ..Config::default()
    };
    let known = |app: &str| ActiveWindow::Known {
        app: app.into(),
        title: TITLE.into(),
    };
    let mono = FakeScreen::single_monitor;

    // Success.
    let mut ok = runner(
        dir.path(),
        FakeWindow::always(known("Other")),
        false,
        mono(),
    );
    assert!(matches!(ok.run_once(&config, NOW), Outcome::Captured(_)));
    // Skips: excluded, excluded during capture, unknown, locked, paused.
    runner(dir.path(), FakeWindow::always(known(APP)), false, mono()).run_once(&config, NOW);
    runner(
        dir.path(),
        FakeWindow::sequence(vec![known("Other"), known(APP)]),
        false,
        mono(),
    )
    .run_once(&config, NOW);
    runner(
        dir.path(),
        FakeWindow::always(ActiveWindow::Unknown),
        false,
        mono(),
    )
    .run_once(&config, NOW);
    runner(dir.path(), FakeWindow::always(known("Other")), true, mono()).run_once(&config, NOW);
    let paused = Config {
        paused: true,
        ..config.clone()
    };
    runner(
        dir.path(),
        FakeWindow::always(known("Other")),
        false,
        mono(),
    )
    .run_once(&paused, NOW);
    // Errors: portal, disk full (the error carries the app name in its path).
    runner(
        dir.path(),
        FakeWindow::always(known("Other")),
        false,
        FakeScreen::failing(CaptureError::Portal),
    )
    .run_once(&config, NOW);
    let mut disk = CycleRunner::new(
        Box::new(mono()),
        Box::new(FakeWindow::always(known("Other"))),
        Box::new(FakeLock::new(false)),
        Box::new(DiskFullSink),
    );
    disk.run_once(&config, NOW);

    let log = String::from_utf8(buf.0.lock().unwrap().clone()).unwrap();
    for expected in [
        "excluded",
        "app-unknown",
        "locked",
        "paused",
        "Portal",
        "DiskFull",
    ] {
        assert!(log.contains(expected), "missing {expected}: {log}");
    }
    for secret in [APP, TITLE, "secretapp"] {
        assert!(
            !log.to_lowercase().contains(&secret.to_lowercase()),
            "log leaks {secret}: {log}"
        );
    }
}
