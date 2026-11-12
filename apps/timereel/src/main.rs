//! `timereel`: the viewer. Starts the window, or brings the running one to
//! the front.

use timereel::app::ViewerApp;
use timereel::single::{self, Claim};
use timereel::sys;

fn main() -> eframe::Result {
    if std::env::args().nth(1).as_deref() == Some("--version") {
        println!("{} {}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"));
        return Ok(());
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
