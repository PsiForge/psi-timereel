//! Logging to the user journal, with stderr as the fallback
//! (architecture section 2.2). Log lines carry reasons and error kinds only.

use tracing::level_filters::LevelFilter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

/// Starts logging at level `info`, or the level in `TIMEREEL_LOG`
/// (`error`, `warn`, `info`, `debug`, `trace`). Uses `tracing-journald` if the journal socket can be
/// opened, and `fmt` on stderr otherwise. Returns where it logs, for one
/// startup line. Calling it twice is harmless (the second call does nothing).
pub fn init() -> &'static str {
    let filter = std::env::var("TIMEREEL_LOG")
        .ok()
        .and_then(|v| v.parse::<LevelFilter>().ok())
        .unwrap_or(LevelFilter::INFO);
    match tracing_journald::layer() {
        Ok(journald) => {
            let _ = tracing_subscriber::registry()
                .with(filter)
                .with(journald)
                .try_init();
            "journal"
        }
        Err(_) => {
            let _ = tracing_subscriber::registry()
                .with(filter)
                .with(tracing_subscriber::fmt::layer().with_writer(std::io::stderr))
                .try_init();
            "stderr"
        }
    }
}
