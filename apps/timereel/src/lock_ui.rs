//! The lock screen and, after a recovery-key unlock, the offer to set a new
//! password. Argon2 runs on worker threads; the UI thread only polls.

use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, TryRecvError};
use std::time::Duration;

use egui::{Button, Color32, RichText, TextEdit};
use timereel_crypto::{
    change_password, check_password, load, unlock_with_password, unlock_with_recovery_key,
    SecretString, UnlockedKey,
};
use zeroize::Zeroizing;

use crate::setup_ui::MISMATCH_MESSAGE;

/// The one message for every failed unlock.
pub const WRONG_SECRET_MESSAGE: &str = "Password or recovery key is not correct";
/// The toggle between password and recovery key.
pub const RECOVERY_TOGGLE_LABEL: &str = "Use recovery key";
/// The unlock button.
pub const UNLOCK_LABEL: &str = "Unlock";
/// Heading of the offer shown after a recovery-key unlock.
pub const NEW_PASSWORD_OFFER: &str = "Set a new password";
/// Button that saves the new password.
pub const SET_PASSWORD_LABEL: &str = "Save new password";
/// Button that skips the offer.
pub const NOT_NOW_LABEL: &str = "Not now";
const ERROR_COLOR: Color32 = Color32::from_rgb(200, 40, 40);
const POLL: Duration = Duration::from_millis(50);

/// A successful unlock.
pub struct Unlocked {
    /// The opened key.
    pub key: UnlockedKey,
    /// True if the recovery key was used.
    pub via_recovery: bool,
}

/// The lock screen.
pub struct LockScreen {
    keyring_path: PathBuf,
    secret: Zeroizing<String>,
    use_recovery: bool,
    error: bool,
    worker: Option<Receiver<Option<UnlockedKey>>>,
}

impl LockScreen {
    /// A lock screen for the key file at `keyring_path`.
    pub fn new(keyring_path: PathBuf) -> Self {
        Self {
            keyring_path,
            secret: Zeroizing::default(),
            use_recovery: false,
            error: false,
            worker: None,
        }
    }

    /// Draws the screen; returns the key once an unlock succeeded.
    pub fn ui(&mut self, ui: &mut egui::Ui) -> Option<Unlocked> {
        if let Some(found) = self.poll(ui.ctx()) {
            return Some(found);
        }
        let busy = self.worker.is_some();
        ui.heading("TimeReel is locked");
        ui.add_space(8.0);
        let hint = if self.use_recovery {
            "Recovery key"
        } else {
            "Password"
        };
        let field = ui.add_enabled(
            !busy,
            TextEdit::singleline(&mut *self.secret)
                .password(true)
                .hint_text(hint),
        );
        ui.add_enabled(
            !busy,
            egui::Checkbox::new(&mut self.use_recovery, RECOVERY_TOGGLE_LABEL),
        );
        let enter = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        let clicked = ui
            .add_enabled(!busy && !self.secret.is_empty(), Button::new(UNLOCK_LABEL))
            .clicked();
        if busy {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Unlocking…");
            });
        }
        if self.error {
            ui.label(RichText::new(WRONG_SECRET_MESSAGE).color(ERROR_COLOR));
        }
        if !busy && !self.secret.is_empty() && (clicked || enter) {
            self.start(ui.ctx());
        }
        None
    }

    fn start(&mut self, ctx: &egui::Context) {
        let secret = SecretString::from(std::mem::take(&mut *self.secret));
        let path = self.keyring_path.clone();
        let recovery = self.use_recovery;
        let (tx, rx) = channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            // Every failure (missing or broken key file, wrong secret) gets
            // the same message, so nothing tells which part was wrong.
            let key = load(&path).ok().and_then(|keyring| {
                if recovery {
                    unlock_with_recovery_key(&keyring, &secret).ok()
                } else {
                    unlock_with_password(&keyring, &secret).ok()
                }
            });
            let _ = tx.send(key);
            ctx.request_repaint();
        });
        self.error = false;
        self.worker = Some(rx);
    }

    fn poll(&mut self, ctx: &egui::Context) -> Option<Unlocked> {
        let rx = self.worker.as_ref()?;
        let result = match rx.try_recv() {
            Ok(key) => key,
            Err(TryRecvError::Empty) => {
                ctx.request_repaint_after(POLL);
                return None;
            }
            Err(TryRecvError::Disconnected) => None,
        };
        self.worker = None;
        match result {
            Some(key) => Some(Unlocked {
                key,
                via_recovery: self.use_recovery,
            }),
            None => {
                self.error = true;
                None
            }
        }
    }
}

type Change = (UnlockedKey, Result<(), String>);

/// The offer to set a new password after a recovery-key unlock (T05's
/// `change_password`). It holds the key until the user is done.
pub struct NewPasswordScreen {
    keyring_path: PathBuf,
    key: Option<UnlockedKey>,
    password: Zeroizing<String>,
    confirm: Zeroizing<String>,
    error: Option<String>,
    worker: Option<Receiver<Change>>,
}

impl NewPasswordScreen {
    /// An offer for the key file at `keyring_path`.
    pub fn new(keyring_path: PathBuf, key: UnlockedKey) -> Self {
        Self {
            keyring_path,
            key: Some(key),
            password: Zeroizing::default(),
            confirm: Zeroizing::default(),
            error: None,
            worker: None,
        }
    }

    /// Draws the offer; returns the key when the user is done (password
    /// saved or skipped).
    pub fn ui(&mut self, ui: &mut egui::Ui) -> Option<UnlockedKey> {
        if let Some(rx) = &self.worker {
            match rx.try_recv() {
                Ok((key, result)) => {
                    self.worker = None;
                    match result {
                        Ok(()) => return Some(key),
                        Err(e) => {
                            self.key = Some(key);
                            self.error = Some(e);
                        }
                    }
                }
                Err(TryRecvError::Empty) => ui.ctx().request_repaint_after(POLL),
                Err(TryRecvError::Disconnected) => {
                    // The worker panicked and took the key with it; the user
                    // has to unlock again.
                    self.worker = None;
                    self.error = Some("The password could not be changed.".into());
                }
            }
        }
        let busy = self.worker.is_some();
        ui.heading(NEW_PASSWORD_OFFER);
        ui.label("You unlocked with the recovery key. You can choose a new password now.");
        ui.add_space(8.0);
        ui.add_enabled(
            !busy,
            TextEdit::singleline(&mut *self.password)
                .password(true)
                .hint_text("New password"),
        );
        ui.add_enabled(
            !busy,
            TextEdit::singleline(&mut *self.confirm)
                .password(true)
                .hint_text("Repeat the new password"),
        );
        let mismatch = !self.confirm.is_empty() && *self.password != *self.confirm;
        if mismatch {
            ui.label(RichText::new(MISMATCH_MESSAGE).color(ERROR_COLOR));
        } else if let Some(warning) = check_password(&self.password).warning() {
            ui.label(warning);
        }
        let ready = !busy && self.key.is_some() && !self.password.is_empty() && !mismatch;
        let ready = ready && *self.password == *self.confirm;
        let mut done = None;
        ui.horizontal(|ui| {
            if ui
                .add_enabled(ready, Button::new(SET_PASSWORD_LABEL))
                .clicked()
            {
                self.start(ui.ctx());
            }
            if ui.add_enabled(!busy, Button::new(NOT_NOW_LABEL)).clicked() {
                done = self.key.take();
            }
        });
        if busy {
            ui.spinner();
        }
        if let Some(e) = &self.error {
            ui.label(RichText::new(e.as_str()).color(ERROR_COLOR));
        }
        done
    }

    fn start(&mut self, ctx: &egui::Context) {
        let Some(key) = self.key.take() else { return };
        let password = SecretString::from(std::mem::take(&mut *self.password));
        self.confirm.clear();
        let path = self.keyring_path.clone();
        let (tx, rx) = channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let result = change_password(&path, &key, &password)
                .map(|_| ())
                .map_err(|e| format!("The new password could not be saved: {e}"));
            let _ = tx.send((key, result));
            ctx.request_repaint();
        });
        self.error = None;
        self.worker = Some(rx);
    }
}
