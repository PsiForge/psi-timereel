//! The viewer window: routing between set-up, lock screen and the unlocked
//! session, auto-lock, and the D-Bus requests.
//!
//! [`Viewer`] holds all of it and draws into any [`egui::Ui`], so tests run
//! it headless; [`ViewerApp`] is the thin `eframe` wrapper.

use std::path::PathBuf;
use std::time::Duration;

use crate::autolock::{autolock_after, Clock, IdleTimer, SystemClock};
use crate::lock_ui::{LockScreen, NewPasswordScreen, Unlocked};
use crate::lockwatch::LockWatch;
use crate::session::Session;
use crate::setup::{self, SetupOptions};
use crate::setup_ui::{SetupScreen, SetupStatus};
use crate::single::{Command, Primary};

/// The text shown where the timeline will be (later tickets).
pub const TIMELINE_PLACEHOLDER: &str = "The timeline comes in a later version.";
/// The button that locks the viewer.
pub const LOCK_LABEL: &str = "Lock";
/// The longest the UI sleeps while unlocked, so a lock change is seen soon.
const MAX_SLEEP: Duration = Duration::from_secs(1);

/// Settings of a [`Viewer`].
pub struct ViewerOptions {
    /// The key file.
    pub keyring_path: PathBuf,
    /// Lock after this long without input.
    pub autolock: Duration,
    /// The time source for auto-lock.
    pub clock: Box<dyn Clock>,
    /// Used when set-up runs.
    pub setup: SetupOptions,
}

impl ViewerOptions {
    /// The real paths and `ui_autolock_minutes` from the config (the default
    /// of 10 minutes if the config cannot be read).
    ///
    /// # Errors
    /// Fails if the data directory cannot be determined.
    pub fn from_env() -> Result<Self, setup::SetupError> {
        let minutes = timereel_config::load()
            .map(|c| c.ui_autolock_minutes)
            .unwrap_or_else(|_| timereel_config::Config::default().ui_autolock_minutes);
        Ok(Self {
            keyring_path: setup::keyring_path()?,
            autolock: autolock_after(minutes),
            clock: Box::new(SystemClock),
            setup: SetupOptions::default(),
        })
    }
}

enum Screen {
    Setup(Box<SetupScreen>),
    Locked(LockScreen),
    /// After a recovery-key unlock: offer a new password.
    NewPassword(NewPasswordScreen),
    Unlocked(Session),
}

/// The viewer without the window around it.
pub struct Viewer {
    screen: Screen,
    options: ViewerOptions,
    idle: IdleTimer,
    primary: Option<Primary>,
    lock_watch: Option<LockWatch>,
}

impl Viewer {
    /// Starts on set-up if there is no key file, otherwise on the lock screen.
    pub fn new(options: ViewerOptions) -> Self {
        let screen = if options.keyring_path.exists() {
            Screen::Locked(LockScreen::new(options.keyring_path.clone()))
        } else {
            Screen::Setup(Box::new(SetupScreen::new(options.setup.clone())))
        };
        let idle = IdleTimer::new(options.autolock, options.clock.now());
        Self {
            screen,
            options,
            idle,
            primary: None,
            lock_watch: None,
        }
    }

    /// Serves `Activate` and `Quit` from this D-Bus name owner.
    pub fn attach_primary(&mut self, primary: Primary) {
        self.primary = Some(primary);
    }

    /// Locks when the desktop session locks.
    pub fn attach_lock_watch(&mut self, watch: LockWatch) {
        self.lock_watch = Some(watch);
    }

    /// Lets the D-Bus services wake up the UI.
    pub fn attach_context(&self, ctx: &egui::Context) {
        if let Some(primary) = &self.primary {
            primary.attach(ctx);
        }
        if let Some(watch) = &self.lock_watch {
            watch.attach(ctx);
        }
    }

    /// True while the lock screen is shown.
    pub fn is_locked(&self) -> bool {
        matches!(self.screen, Screen::Locked(_))
    }

    /// True while decrypted state exists (the timeline or the new-password
    /// offer).
    pub fn is_unlocked(&self) -> bool {
        matches!(self.screen, Screen::Unlocked(_) | Screen::NewPassword(_))
    }

    /// The unlocked session, for the timeline to register its hooks with.
    pub fn session_mut(&mut self) -> Option<&mut Session> {
        match &mut self.screen {
            Screen::Unlocked(session) => Some(session),
            _ => None,
        }
    }

    /// Drops all decrypted state and shows the lock screen. Does nothing
    /// during set-up or when already locked.
    pub fn lock(&mut self) {
        if !self.is_unlocked() {
            return;
        }
        let old = std::mem::replace(
            &mut self.screen,
            Screen::Locked(LockScreen::new(self.options.keyring_path.clone())),
        );
        if let Screen::Unlocked(mut session) = old {
            session.lock();
        }
    }

    /// Draws one frame.
    pub fn ui(&mut self, ui: &mut egui::Ui) {
        self.handle_commands(ui);
        let now = self.options.clock.now();
        let input = ui.input(|i| {
            i.events
                .iter()
                .any(|e| !matches!(e, egui::Event::WindowFocused(_)))
        });
        if input {
            self.idle.input(now);
        }
        let session_locked = self
            .lock_watch
            .as_ref()
            .is_some_and(LockWatch::session_locked);
        if session_locked || self.idle.expired(now) {
            self.lock();
        }
        if self.is_unlocked() {
            ui.ctx()
                .request_repaint_after(self.idle.remaining(now).min(MAX_SLEEP));
        }

        let mut next = None;
        let mut lock = false;
        match &mut self.screen {
            Screen::Setup(setup) => {
                if setup.ui(ui) == SetupStatus::Finished {
                    next = Some(Screen::Locked(LockScreen::new(
                        self.options.keyring_path.clone(),
                    )));
                }
            }
            Screen::Locked(screen) => {
                if let Some(Unlocked { key, via_recovery }) = screen.ui(ui) {
                    next = Some(if via_recovery {
                        Screen::NewPassword(NewPasswordScreen::new(
                            self.options.keyring_path.clone(),
                            key,
                        ))
                    } else {
                        Screen::Unlocked(Session::new(key))
                    });
                }
            }
            Screen::NewPassword(offer) => {
                if let Some(key) = offer.ui(ui) {
                    next = Some(Screen::Unlocked(Session::new(key)));
                }
            }
            Screen::Unlocked(_) => {
                ui.horizontal(|ui| {
                    ui.heading("TimeReel");
                    lock = ui.button(LOCK_LABEL).clicked();
                });
                ui.label(TIMELINE_PLACEHOLDER);
            }
        }
        if let Some(next) = next {
            self.screen = next;
            self.idle.input(now);
        }
        if lock {
            self.lock();
        }
    }

    fn handle_commands(&mut self, ui: &egui::Ui) {
        let Some(primary) = &self.primary else { return };
        while let Some(command) = primary.try_next() {
            match command {
                // While locked the lock screen is what comes to the front.
                Command::Activate => {
                    ui.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                    ui.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                    ui.send_viewport_cmd(egui::ViewportCommand::Focus);
                }
                Command::Quit => ui.send_viewport_cmd(egui::ViewportCommand::Close),
            }
        }
    }
}

/// The `eframe` application.
pub struct ViewerApp {
    viewer: Viewer,
}

impl ViewerApp {
    /// Picks the first screen: no key file means set-up, otherwise the lock
    /// screen. Starts following the desktop lock state.
    ///
    /// # Errors
    /// Fails if the data directory cannot be determined.
    pub fn new(primary: Primary, ctx: &egui::Context) -> Result<Self, setup::SetupError> {
        let mut viewer = Viewer::new(ViewerOptions::from_env()?);
        viewer.attach_primary(primary);
        match LockWatch::start_default() {
            Ok(watch) => viewer.attach_lock_watch(watch),
            Err(e) => eprintln!("timereel: cannot follow the desktop lock state: {e}"),
        }
        viewer.attach_context(ctx);
        Ok(Self { viewer })
    }
}

impl eframe::App for ViewerApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ui, |ui| self.viewer.ui(ui));
    }
}
