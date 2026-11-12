//! The viewer window: routing at start-up and the D-Bus requests.

use crate::setup::{self, SetupOptions};
use crate::setup_ui::{SetupScreen, SetupStatus};
use crate::single::{Command, Primary};

enum Screen {
    Setup(Box<SetupScreen>),
    /// Placeholder until the lock screen (T17).
    Locked,
}

/// The `eframe` application.
pub struct ViewerApp {
    screen: Screen,
    primary: Primary,
}

impl ViewerApp {
    /// Picks the first screen: no key file means set-up, otherwise the lock
    /// screen.
    ///
    /// # Errors
    /// Fails if the data directory cannot be determined.
    pub fn new(primary: Primary, ctx: &egui::Context) -> Result<Self, setup::SetupError> {
        primary.attach(ctx);
        let screen = if setup::is_set_up()? {
            Screen::Locked
        } else {
            Screen::Setup(Box::new(SetupScreen::new(SetupOptions::default())))
        };
        Ok(Self { screen, primary })
    }
}

impl eframe::App for ViewerApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        while let Some(command) = self.primary.try_next() {
            match command {
                Command::Activate => {
                    ui.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                    ui.send_viewport_cmd(egui::ViewportCommand::Focus);
                }
                Command::Quit => ui.send_viewport_cmd(egui::ViewportCommand::Close),
            }
        }
        egui::CentralPanel::default().show(ui, |ui| match &mut self.screen {
            Screen::Setup(setup) => {
                if setup.ui(ui) == SetupStatus::Finished {
                    self.screen = Screen::Locked;
                }
            }
            Screen::Locked => {
                ui.heading("TimeReel");
                ui.label("Locked. The lock screen comes in a later version.");
            }
        });
    }
}
