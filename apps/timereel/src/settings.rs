//! The Settings screen (architecture section 2.4).
//!
//! [`SettingsScreen`] edits the capture settings, saves them with
//! `timereel_config::update` and tells the agent with `ReloadConfig`. Lowering
//! the retention time first asks for a confirmation that names the number of
//! captures that will be deleted, and runs retention after the save. The
//! Change password dialog uses the current password or the recovery key and
//! `timereel_crypto::change_password`.
//!
//! Validation messages come from `timereel-config`; this module only adds the
//! message for text that is not a number. The form keeps numbers as text so
//! that an invalid entry can be shown and refused instead of being corrected
//! silently.

use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, TryRecvError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use egui::{Button, Color32, RichText, TextEdit};
use timereel_config::{Config, FieldError, ImageFormat, MAX_EXCLUDED_APPS};
use timereel_crypto::{
    change_password, check_password, load, unlock_with_password, unlock_with_recovery_key,
    SecretString,
};
use timereel_store::retention::count_older_than;
use timereel_store::{CaptureTime, Store};
use zeroize::Zeroizing;

use crate::agent_client::{AgentControl, AgentError, DbusAgent};
use crate::lock_ui::WRONG_SECRET_MESSAGE;
use crate::setup_ui::MISMATCH_MESSAGE;

/// Heading of the screen.
pub const SETTINGS_HEADING: &str = "Settings";
/// Button of the main view that opens the screen.
pub const SETTINGS_LABEL: &str = "Settings";
/// Button that leaves the screen.
pub const BACK_LABEL: &str = "Back";
/// Button that saves the form.
pub const SAVE_LABEL: &str = "Save";
/// Button that opens the Change password dialog.
pub const CHANGE_PASSWORD_LABEL: &str = "Change password…";
/// Button of the dialog that changes the password.
pub const CHANGE_PASSWORD_SUBMIT_LABEL: &str = "Change password";
/// Toggle of the dialog: recovery key instead of the current password.
pub const USE_RECOVERY_LABEL: &str = "Use recovery key";
/// Button that adds a row to the excluded apps.
pub const ADD_APP_LABEL: &str = "Add app";
/// Button that removes a row of the excluded apps.
pub const REMOVE_APP_LABEL: &str = "Remove";
/// Button that confirms deleting old captures.
pub const CONFIRM_DELETE_LABEL: &str = "Save and delete";
/// Button that cancels the confirmation or the dialog.
pub const CANCEL_LABEL: &str = "Cancel";
/// Label of the pause toggle.
pub const PAUSE_LABEL: &str = "Pause recording";
/// Label of the toggle for unknown apps.
pub const CAPTURE_UNKNOWN_LABEL: &str = "Capture when the active app is unknown";
/// Privacy warning next to that toggle.
pub const CAPTURE_UNKNOWN_WARNING: &str =
    "Privacy: if the active app cannot be determined, an excluded app (for example a password \
     manager) could be captured. Leave this off unless you need it.";
/// Hint for the excluded apps.
pub const EXCLUDED_HINT: &str =
    "App names that are never captured, for example KeePassXC, 1Password, Bitwarden.";
/// Shown after a save when the agent is not running.
pub const SAVED_OFFLINE_MESSAGE: &str = "Saved; applies when the agent starts";
/// Shown after a save that the agent has applied.
pub const SAVED_MESSAGE: &str = "Saved and applied.";
/// Shown when Save is refused.
pub const REFUSED_MESSAGE: &str = "Not saved: fix the marked fields first.";
/// Shown after a successful password change.
pub const PASSWORD_CHANGED_MESSAGE: &str = "Password changed.";
/// Shown when pause or resume cannot reach the agent.
pub const PAUSE_OFFLINE_MESSAGE: &str =
    "The agent is not running, so recording cannot be paused or resumed now.";

const ERROR_COLOR: Color32 = Color32::from_rgb(200, 40, 40);
const POLL: Duration = Duration::from_millis(50);

/// The order of the single-line fields in the form, for tests that find them
/// by position: interval, width, quality, thumbnail width, retention, auto-lock.
pub const NUMBER_FIELDS: usize = 6;

/// What the screen needs from the outside.
pub struct SettingsOptions {
    /// Where the captures are (for the retention count).
    pub store: Store,
    /// The key file (for Change password).
    pub keyring_path: PathBuf,
    /// The agent.
    pub agent: Box<dyn AgentControl>,
    /// The current time.
    pub now: Box<dyn Fn() -> CaptureTime>,
}

impl SettingsOptions {
    /// The real store, the real agent and the system clock.
    ///
    /// # Errors
    /// Fails if the data directory cannot be determined.
    pub fn from_env(keyring_path: PathBuf) -> Result<Self, timereel_config::PathError> {
        Ok(Self {
            store: Store::at_default_location()?,
            keyring_path,
            agent: Box::new(DbusAgent),
            now: Box::new(system_now),
        })
    }
}

/// The system time as a capture time.
pub fn system_now() -> CaptureTime {
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0);
    CaptureTime(ms)
}

/// The text of the form fields.
#[derive(Clone)]
struct Form {
    interval: String,
    width: String,
    format: ImageFormat,
    quality: String,
    thumbnail: String,
    retention: String,
    apps: Vec<String>,
    capture_unknown: bool,
    autolock: String,
}

impl Form {
    fn from_config(c: &Config) -> Self {
        Self {
            interval: c.interval_minutes.to_string(),
            width: c.width_px.to_string(),
            format: c.format,
            quality: c.quality.to_string(),
            thumbnail: c.thumbnail_width_px.to_string(),
            retention: c.retention_days.to_string(),
            apps: c.excluded_apps.clone(),
            capture_unknown: c.capture_when_app_unknown,
            autolock: c.ui_autolock_minutes.to_string(),
        }
    }

    /// The config this form describes (on top of `base`, which keeps
    /// `paused` and `public_key`), and every problem found. A field that is
    /// not a number keeps the value of `base` and gets its own message.
    fn to_config(&self, base: &Config) -> (Config, Vec<FieldError>) {
        let mut errors = Vec::new();
        let mut number = |field: &str, label: &str, text: &str, old: u32| -> u32 {
            if let Ok(n) = text.trim().parse::<u32>() {
                n
            } else {
                errors.push(FieldError {
                    field: field.to_string(),
                    message: format!("{label} must be a whole number"),
                });
                old
            }
        };
        let mut config = base.clone();
        config.interval_minutes = number(
            "interval_minutes",
            "The capture interval",
            &self.interval,
            base.interval_minutes,
        );
        config.width_px = number("width_px", "The image width", &self.width, base.width_px);
        config.quality = number("quality", "The image quality", &self.quality, base.quality);
        config.thumbnail_width_px = number(
            "thumbnail_width_px",
            "The thumbnail width",
            &self.thumbnail,
            base.thumbnail_width_px,
        );
        config.retention_days = number(
            "retention_days",
            "The retention time",
            &self.retention,
            base.retention_days,
        );
        config.ui_autolock_minutes = number(
            "ui_autolock_minutes",
            "The auto-lock time",
            &self.autolock,
            base.ui_autolock_minutes,
        );
        config.format = self.format;
        config.excluded_apps = self.apps.clone();
        config.capture_when_app_unknown = self.capture_unknown;
        if let Err(found) = config.validate() {
            for e in found.0 {
                if !errors.iter().any(|known| known.field == e.field) {
                    errors.push(e);
                }
            }
        }
        (config, errors)
    }
}

fn error_for<'a>(errors: &'a [FieldError], field: &str) -> Option<&'a FieldError> {
    errors.iter().find(|e| e.field == field)
}

enum Mode {
    Form,
    /// Waiting for the user to confirm deleting `count` captures.
    ConfirmRetention {
        count: usize,
        days: u32,
    },
    Password(Box<PasswordDialog>),
}

/// The Settings screen.
pub struct SettingsScreen {
    options: SettingsOptions,
    /// The config as last read or saved.
    saved: Config,
    form: Form,
    mode: Mode,
    status: Option<(String, bool)>,
}

impl SettingsScreen {
    /// A screen showing the current config (the defaults, with a message, if
    /// the file cannot be read).
    pub fn new(options: SettingsOptions) -> Self {
        let (saved, status) = match timereel_config::load() {
            Ok(c) => (c, None),
            Err(e) => (
                Config::default(),
                Some((format!("The settings could not be read: {e}"), true)),
            ),
        };
        Self {
            options,
            form: Form::from_config(&saved),
            saved,
            mode: Mode::Form,
            status,
        }
    }

    /// The message under the form, for tests.
    pub fn status(&self) -> Option<&str> {
        self.status.as_ref().map(|(text, _)| text.as_str())
    }

    /// The config as last read or saved.
    pub fn saved(&self) -> &Config {
        &self.saved
    }

    /// Draws the screen. Returns the new config after a save, so the viewer
    /// can apply the auto-lock time. The Back button is drawn by the caller.
    pub fn ui(&mut self, ui: &mut egui::Ui) -> Option<Config> {
        let mut saved = None;
        ui.heading(SETTINGS_HEADING);
        match std::mem::replace(&mut self.mode, Mode::Form) {
            Mode::Form => saved = self.form_ui(ui),
            Mode::ConfirmRetention { count, days } => {
                let (stay, confirmed) = self.confirm_ui(ui, count, days);
                if stay {
                    self.mode = Mode::ConfirmRetention { count, days };
                }
                if confirmed {
                    saved = self.apply(true);
                }
            }
            Mode::Password(mut dialog) => match dialog.ui(ui) {
                DialogStatus::Open => self.mode = Mode::Password(dialog),
                DialogStatus::Cancelled => {}
                DialogStatus::Changed => {
                    self.status = Some((PASSWORD_CHANGED_MESSAGE.into(), false));
                }
            },
        }
        saved
    }

    fn confirm_ui(&mut self, ui: &mut egui::Ui, count: usize, days: u32) -> (bool, bool) {
        ui.add_space(8.0);
        let what = if count == 1 {
            "1 capture".to_string()
        } else {
            format!("{count} captures")
        };
        ui.label(format!(
            "Keeping captures for {days} days will delete {what} that {verb} older. \
             This cannot be undone.",
            verb = if count == 1 { "is" } else { "are" },
        ));
        let (mut stay, mut confirmed) = (true, false);
        ui.horizontal(|ui| {
            if ui.button(CONFIRM_DELETE_LABEL).clicked() {
                stay = false;
                confirmed = true;
            }
            if ui.button(CANCEL_LABEL).clicked() {
                // Nothing was written: the old value stays.
                self.form.retention = self.saved.retention_days.to_string();
                self.status = None;
                stay = false;
            }
        });
        (stay, confirmed)
    }

    fn form_ui(&mut self, ui: &mut egui::Ui) -> Option<Config> {
        let (candidate, errors) = self.form.to_config(&self.saved);
        let mut save = false;
        let mut open_password = false;
        let mut pause_toggle = None;
        egui::ScrollArea::vertical().show(ui, |ui| {
            let form = &mut self.form;
            let number = |ui: &mut egui::Ui, label: &str, field: &str, text: &mut String| {
                ui.horizontal(|ui| {
                    let name = ui.label(label);
                    ui.add(TextEdit::singleline(text).desired_width(80.0))
                        .labelled_by(name.id);
                });
                if let Some(e) = error_for(&errors, field) {
                    ui.label(RichText::new(e.message.as_str()).color(ERROR_COLOR));
                }
            };
            number(
                ui,
                "Capture interval (minutes)",
                "interval_minutes",
                &mut form.interval,
            );
            number(ui, "Image width (pixels)", "width_px", &mut form.width);
            number(ui, "Image quality (1-100)", "quality", &mut form.quality);
            number(
                ui,
                "Thumbnail width (pixels)",
                "thumbnail_width_px",
                &mut form.thumbnail,
            );
            number(
                ui,
                "Keep captures (days)",
                "retention_days",
                &mut form.retention,
            );
            number(
                ui,
                "Lock the viewer after (minutes)",
                "ui_autolock_minutes",
                &mut form.autolock,
            );
            ui.horizontal(|ui| {
                ui.label("Image format");
                ui.selectable_value(&mut form.format, ImageFormat::Webp, "WebP");
                ui.selectable_value(&mut form.format, ImageFormat::Jpeg, "JPEG");
            });

            ui.add_space(8.0);
            ui.label(RichText::new("Excluded apps").strong());
            ui.label(EXCLUDED_HINT);
            if let Some(e) = error_for(&errors, "excluded_apps") {
                ui.label(RichText::new(e.message.as_str()).color(ERROR_COLOR));
            }
            let mut remove = None;
            for (i, app) in form.apps.iter_mut().enumerate() {
                ui.horizontal(|ui| {
                    ui.add(TextEdit::singleline(app).hint_text("KeePassXC"));
                    if ui.button(REMOVE_APP_LABEL).clicked() {
                        remove = Some(i);
                    }
                });
                if let Some(e) = error_for(&errors, &format!("excluded_apps[{i}]")) {
                    ui.label(RichText::new(e.message.as_str()).color(ERROR_COLOR));
                }
            }
            if let Some(i) = remove {
                form.apps.remove(i);
            }
            if ui
                .add_enabled(
                    form.apps.len() < MAX_EXCLUDED_APPS,
                    Button::new(ADD_APP_LABEL),
                )
                .clicked()
            {
                form.apps.push(String::new());
            }

            ui.add_space(8.0);
            ui.checkbox(&mut form.capture_unknown, CAPTURE_UNKNOWN_LABEL);
            ui.label(RichText::new(CAPTURE_UNKNOWN_WARNING).color(Color32::from_rgb(170, 100, 0)));

            ui.add_space(8.0);
            let mut paused = self.saved.paused;
            if ui.checkbox(&mut paused, PAUSE_LABEL).changed() {
                pause_toggle = Some(paused);
            }

            ui.add_space(8.0);
            ui.horizontal(|ui| {
                save = ui.button(SAVE_LABEL).clicked();
                open_password = ui.button(CHANGE_PASSWORD_LABEL).clicked();
            });
            if let Some((text, is_error)) = &self.status {
                let color = if *is_error {
                    ERROR_COLOR
                } else {
                    ui.visuals().text_color()
                };
                ui.label(RichText::new(text.as_str()).color(color));
            }
        });

        if let Some(paused) = pause_toggle {
            self.toggle_pause(paused);
        }
        if open_password {
            self.mode = Mode::Password(Box::new(PasswordDialog::new(
                self.options.keyring_path.clone(),
            )));
        }
        if save {
            return self.on_save(candidate, &errors);
        }
        None
    }

    fn toggle_pause(&mut self, paused: bool) {
        match self.options.agent.set_paused(paused) {
            Ok(()) => {
                // The agent saved `paused` to the config; read it back.
                match timereel_config::load() {
                    Ok(c) => self.saved.paused = c.paused,
                    Err(_) => self.saved.paused = paused,
                }
                self.status = None;
            }
            Err(AgentError::NotRunning) => {
                self.status = Some((PAUSE_OFFLINE_MESSAGE.into(), true));
            }
            Err(e) => {
                self.status = Some((format!("Could not change the pause state: {e}"), true));
            }
        }
    }

    fn on_save(&mut self, candidate: Config, errors: &[FieldError]) -> Option<Config> {
        if !errors.is_empty() {
            self.status = Some((REFUSED_MESSAGE.into(), true));
            return None;
        }
        if candidate.retention_days < self.saved.retention_days {
            let count = count_older_than(
                &self.options.store,
                (self.options.now)(),
                candidate.retention_days,
            );
            self.mode = Mode::ConfirmRetention {
                count,
                days: candidate.retention_days,
            };
            self.status = None;
            return None;
        }
        self.apply(false)
    }

    /// Writes the form, then tells the agent. With `run_retention` it also
    /// asks the agent to delete the old captures.
    fn apply(&mut self, run_retention: bool) -> Option<Config> {
        let form = self.form.clone();
        let result = timereel_config::update(|c| {
            // Only the settings of the form: `paused` and `public_key` are
            // not ours and may have changed since the screen was opened.
            let (new, _) = form.to_config(c);
            *c = new;
        });
        let config = match result {
            Ok(c) => c,
            Err(e) => {
                self.status = Some((format!("Not saved: {e}"), true));
                return None;
            }
        };
        self.saved = config.clone();
        self.form = Form::from_config(&config);
        self.status = Some(match self.options.agent.reload_config() {
            Ok(()) if run_retention => match self.options.agent.run_retention() {
                Ok(0) => (format!("{SAVED_MESSAGE} No captures were deleted."), false),
                Ok(n) => (format!("{SAVED_MESSAGE} {n} captures were deleted."), false),
                Err(e) => (
                    format!("Saved, but old captures could not be deleted now: {e}"),
                    true,
                ),
            },
            Ok(()) => (SAVED_MESSAGE.to_string(), false),
            Err(AgentError::NotRunning) if run_retention => (
                format!("{SAVED_OFFLINE_MESSAGE}; old captures are deleted then"),
                false,
            ),
            Err(AgentError::NotRunning) => (SAVED_OFFLINE_MESSAGE.to_string(), false),
            Err(AgentError::Failed(text)) => (
                format!("Saved, but the agent did not apply it: {text}"),
                true,
            ),
        });
        Some(config)
    }
}

enum DialogStatus {
    Open,
    Cancelled,
    Changed,
}

/// The Change password dialog.
struct PasswordDialog {
    keyring_path: PathBuf,
    current: Zeroizing<String>,
    use_recovery: bool,
    new: Zeroizing<String>,
    confirm: Zeroizing<String>,
    error: Option<String>,
    worker: Option<Receiver<Result<(), String>>>,
}

impl PasswordDialog {
    fn new(keyring_path: PathBuf) -> Self {
        Self {
            keyring_path,
            current: Zeroizing::default(),
            use_recovery: false,
            new: Zeroizing::default(),
            confirm: Zeroizing::default(),
            error: None,
            worker: None,
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui) -> DialogStatus {
        if let Some(rx) = &self.worker {
            match rx.try_recv() {
                Ok(Ok(())) => {
                    self.worker = None;
                    return DialogStatus::Changed;
                }
                Ok(Err(e)) => {
                    self.worker = None;
                    self.error = Some(e);
                }
                Err(TryRecvError::Empty) => ui.ctx().request_repaint_after(POLL),
                Err(TryRecvError::Disconnected) => {
                    self.worker = None;
                    self.error = Some("The password could not be changed.".into());
                }
            }
        }
        let busy = self.worker.is_some();
        ui.add_space(8.0);
        ui.label(RichText::new("Change your password").strong());
        ui.add_enabled(
            !busy,
            TextEdit::singleline(&mut *self.current)
                .password(true)
                .hint_text(if self.use_recovery {
                    "Recovery key"
                } else {
                    "Current password"
                }),
        );
        ui.add_enabled(
            !busy,
            egui::Checkbox::new(&mut self.use_recovery, USE_RECOVERY_LABEL),
        );
        ui.add_enabled(
            !busy,
            TextEdit::singleline(&mut *self.new)
                .password(true)
                .hint_text("New password"),
        );
        ui.add_enabled(
            !busy,
            TextEdit::singleline(&mut *self.confirm)
                .password(true)
                .hint_text("Repeat the new password"),
        );
        let mismatch = !self.confirm.is_empty() && *self.new != *self.confirm;
        if mismatch {
            ui.label(RichText::new(MISMATCH_MESSAGE).color(ERROR_COLOR));
        } else if let Some(warning) = check_password(&self.new).warning() {
            ui.label(warning);
        }
        let ready =
            !busy && !self.current.is_empty() && !self.new.is_empty() && *self.new == *self.confirm;
        let mut status = DialogStatus::Open;
        ui.horizontal(|ui| {
            // A click on a disabled button does nothing, so mismatched or
            // missing input never reaches the key file.
            if ui
                .add_enabled(ready, Button::new(CHANGE_PASSWORD_SUBMIT_LABEL))
                .clicked()
            {
                self.start(ui.ctx());
            }
            if ui.add_enabled(!busy, Button::new(CANCEL_LABEL)).clicked() {
                status = DialogStatus::Cancelled;
            }
        });
        if busy {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Changing the password…");
            });
        }
        if let Some(e) = &self.error {
            ui.label(RichText::new(e.as_str()).color(ERROR_COLOR));
        }
        status
    }

    fn start(&mut self, ctx: &egui::Context) {
        let current = SecretString::from(std::mem::take(&mut *self.current));
        let new = SecretString::from(std::mem::take(&mut *self.new));
        self.confirm.clear();
        let path = self.keyring_path.clone();
        let recovery = self.use_recovery;
        let (tx, rx) = channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let result = (|| {
                let keyring = load(&path).map_err(|_| WRONG_SECRET_MESSAGE.to_string())?;
                let key = if recovery {
                    unlock_with_recovery_key(&keyring, &current)
                } else {
                    unlock_with_password(&keyring, &current)
                }
                .map_err(|_| WRONG_SECRET_MESSAGE.to_string())?;
                change_password(&path, &key, &new)
                    .map(|_| ())
                    .map_err(|e| format!("The new password could not be saved: {e}"))
            })();
            let _ = tx.send(result);
            ctx.request_repaint();
        });
        self.error = None;
        self.worker = Some(rx);
    }
}
