//! `timereel`: the viewer. Starts the window, or brings the running one to
//! the front. `timereel purge-data` deletes all TimeReel data instead.

use timereel::app::ViewerApp;
use timereel::purge::{self, Outcome, PurgeOptions};
use timereel::single::{self, Claim};
use timereel::sys;

fn main() -> eframe::Result {
    if std::env::args().nth(1).as_deref() == Some("--version") {
        println!("{} {}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    match std::env::args().nth(1).as_deref() {
        Some("purge-data") => return purge_data(),
        Some(other) => {
            eprintln!(
                "timereel: unknown argument \"{other}\"\nUsage: timereel [purge-data | --version]"
            );
            std::process::exit(2);
        }
        None => {}
    }
    if let Err(e) = sys::disable_dumping() {
        eprintln!("timereel: could not turn off core dumps: {e}");
    }
    let primary = match single::claim() {
        Ok(Claim::Primary(primary)) => primary,
        Ok(Claim::AlreadyRunning) => {
            if let Err(e) = single::activate_running() {
                eprintln!("timereel: another TimeReel is running but did not answer: {e}");
                std::process::exit(1);
            }
            return Ok(());
        }
        Err(e) => {
            eprintln!("timereel: cannot reach the session bus: {e}");
            std::process::exit(1);
        }
    };
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("TimeReel")
            .with_app_id("org.timereel.Viewer")
            .with_inner_size([960.0, 640.0])
            .with_min_inner_size([480.0, 360.0]),
        ..Default::default()
    };
    eframe::run_native(
        "TimeReel",
        options,
        Box::new(|cc| {
            ViewerApp::new(primary, &cc.egui_ctx)
                .map(|app| Box::new(app) as Box<dyn eframe::App>)
                .map_err(Into::into)
        }),
    )
}

/// `timereel purge-data`: asks on the terminal, then deletes. Never opens a window.
fn purge_data() -> eframe::Result {
    if std::env::args().nth(2).is_some() {
        eprintln!("timereel: purge-data takes no arguments");
        std::process::exit(2);
    }
    let result = PurgeOptions::from_env().and_then(|options| {
        purge::run(
            &options,
            &mut std::io::stdin().lock(),
            &mut std::io::stdout(),
        )
    });
    match result {
        Ok(Outcome::Purged) => Ok(()),
        Ok(Outcome::Cancelled) => std::process::exit(1),
        Err(e) => {
            eprintln!("timereel: {e}");
            std::process::exit(1);
        }
    }
}
