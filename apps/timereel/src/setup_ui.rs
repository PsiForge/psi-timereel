//! The first-run set-up screen: choose a password, create the key, show the
//! recovery key once, then store the key and start the capture service.
//!
//! Key creation (Argon2, seconds) and the installation (fsync, `systemctl`)
//! run on worker threads; the UI thread only polls their channels.

use std::sync::mpsc::{channel, Receiver, TryRecvError};
use std::time::Duration;

use egui::{Button, Color32, RichText, TextEdit};
use timereel_crypto::{check_password, ExposeSecret, Keyring, SecretString};
use zeroize::{Zeroize, Zeroizing};

use crate::setup::{self, NewKey, SetupOptions};

/// Shown when the two password fields differ.
pub const MISMATCH_MESSAGE: &str = "The two passwords do not match.";
/// The tick box that must be set before the recovery screen can be left.
pub const STORED_LABEL: &str = "I have stored my recovery key";
/// Label of the button that moves on.
pub const CONTINUE_LABEL: &str = "Continue";
/// Number of 4-character groups per line of the recovery key.
const GROUPS_PER_LINE: usize = 4;
const WARN_COLOR: Color32 = Color32::from_rgb(200, 120, 0);
const ERROR_COLOR: Color32 = Color32::from_rgb(200, 40, 40);

/// What the screen wants the shell to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetupStatus {
    /// Still on one of the set-up pages.
    Running,
    /// Set-up is over; go on to the next screen.
    Finished,
}

type Worker<T> = Receiver<Result<T, String>>;

enum Phase {
    Form,
    Creating(Worker<NewKey>),
    ShowKey(ShowKey),
    Installing(ShowKey, Worker<Option<String>>),
    Done {
        agent_error: Option<String>,
        retry: Option<Worker<()>>,
    },
}

struct ShowKey {
    keyring: Keyring,
    text: SecretString,
    stored: bool,
    error: Option<String>,
}

/// The set-up pages as one widget. Nothing is written to disk until the user
/// has ticked the recovery key box and pressed Continue.
pub struct SetupScreen {
    options: SetupOptions,
    password: String,
    confirm: String,
    error: Option<String>,
    phase: Phase,
}

impl SetupScreen {
    /// A screen on its first page.
    pub fn new(options: SetupOptions) -> Self {
        Self {
            options,
            password: String::new(),
            confirm: String::new(),
            error: None,
            phase: Phase::Form,
        }
    }

    /// Draws the current page and handles its buttons.
    pub fn ui(&mut self, ui: &mut egui::Ui) -> SetupStatus {
        self.poll(ui.ctx());
        ui.heading("Set up TimeReel");
        ui.add_space(8.0);
        match &self.phase {
            Phase::Form => self.form_ui(ui),
            Phase::Creating(_) => {
                busy(ui, "Creating your key. This takes a few seconds...");
            }
            Phase::ShowKey(_) => self.show_key_ui(ui),
            Phase::Installing(..) => busy(ui, "Saving and starting TimeReel..."),
            Phase::Done { .. } => return self.done_ui(ui),
        }
        SetupStatus::Running
    }

    /// Takes in the result of a finished worker.
    fn poll(&mut self, ctx: &egui::Context) {
        let phase = std::mem::replace(&mut self.phase, Phase::Form);
        self.phase = match phase {
            Phase::Creating(rx) => match poll_worker(&rx, ctx) {
                Poll::Waiting => Phase::Creating(rx),
                Poll::Ready(Ok(key)) => self.password_is_used(key),
                Poll::Ready(Err(message)) => {
                    self.error = Some(message);
                    Phase::Form
                }
            },
            Phase::Installing(mut shown, rx) => match poll_worker(&rx, ctx) {
                Poll::Waiting => Phase::Installing(shown, rx),
                Poll::Ready(Ok(agent_error)) => Phase::Done {
                    agent_error,
                    retry: None,
                },
                Poll::Ready(Err(message)) => {
                    shown.error = Some(message);
                    Phase::ShowKey(shown)
                }
            },
            Phase::Done {
                agent_error,
                retry: Some(rx),
            } => match poll_worker(&rx, ctx) {
                Poll::Waiting => Phase::Done {
                    agent_error,
                    retry: Some(rx),
                },
                Poll::Ready(result) => Phase::Done {
                    agent_error: result.err(),
                    retry: None,
                },
            },
            other => other,
        };
    }

    /// The key exists: forget the typed passwords and show the recovery key.
    fn password_is_used(&mut self, key: NewKey) -> Phase {
        self.password.zeroize();
        self.confirm.zeroize();
        self.error = None;
        Phase::ShowKey(ShowKey {
            text: key.recovery_text(),
            keyring: key.keyring().clone(),
            stored: false,
            error: None,
        })
    }

    fn form_ui(&mut self, ui: &mut egui::Ui) {
        ui.label(
            "Choose a password. You need it every time you open TimeReel to look at your \
             screenshots. It cannot be reset: if you lose it, only the recovery key on the \
             next page can open your data.",
        );
        ui.add_space(8.0);
        let label = ui.label("Password");
        ui.add(
            TextEdit::singleline(&mut self.password)
                .password(true)
                .id_salt("setup-password"),
        )
        .labelled_by(label.id);
        let label = ui.label("Repeat the password");
        let confirm = ui
            .add(
                TextEdit::singleline(&mut self.confirm)
                    .password(true)
                    .id_salt("setup-confirm"),
            )
            .labelled_by(label.id);
        let mismatch = !self.confirm.is_empty() && self.password != self.confirm;
        if mismatch {
            ui.colored_label(ERROR_COLOR, MISMATCH_MESSAGE);
        }
        if !self.password.is_empty() {
            if let Some(warning) = check_password(&self.password).warning() {
                ui.colored_label(WARN_COLOR, warning);
            }
        }
        if let Some(error) = &self.error {
            ui.colored_label(ERROR_COLOR, error);
        }
        ui.add_space(8.0);
        let can_continue = !self.password.is_empty() && self.password == self.confirm;
        let pressed_enter = confirm.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        let clicked = ui
            .add_enabled(can_continue, Button::new(CONTINUE_LABEL))
            .clicked();
        if can_continue && (clicked || pressed_enter) {
            self.start_creating(ui.ctx());
        }
    }

    fn start_creating(&mut self, ctx: &egui::Context) {
        let password = SecretString::from(self.password.clone());
        let options = self.options.clone();
        let (tx, rx) = channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let result = setup::create_key(&password, &options).map_err(|e| e.to_string());
            // The receiver is gone only if the window was closed.
            let _ = tx.send(result);
            ctx.request_repaint();
        });
        self.error = None;
        self.phase = Phase::Creating(rx);
    }

    fn show_key_ui(&mut self, ui: &mut egui::Ui) {
        let Phase::ShowKey(shown) = &mut self.phase else {
            return;
        };
        ui.label(
            "This is your recovery key. If you forget your password, it is the only way to \
             open your screenshots. It is shown once and TimeReel does not keep it.",
        );
        ui.add_space(4.0);
        ui.label(
            "Write it on paper, or save it in a password manager. Keep it somewhere other than \
             this computer. Do not save it in a plain file or send it to yourself.",
        );
        ui.add_space(8.0);
        let lines = Zeroizing::new(recovery_lines(shown.text.expose_secret()));
        egui::Frame::group(ui.style()).show(ui, |ui| {
            for line in lines.iter() {
                // Not selectable, so it cannot be copied to the clipboard by accident.
                ui.add(
                    egui::Label::new(RichText::new(line.as_str()).monospace().size(20.0))
                        .selectable(false),
                );
            }
        });
        ui.add_space(8.0);
        ui.checkbox(&mut shown.stored, STORED_LABEL);
        if let Some(error) = &shown.error {
            ui.colored_label(ERROR_COLOR, error);
        }
        ui.add_space(8.0);
        let clicked = ui
            .add_enabled(shown.stored, Button::new(CONTINUE_LABEL))
            .clicked();
        if clicked {
            self.start_installing(ui.ctx());
        }
    }

    fn start_installing(&mut self, ctx: &egui::Context) {
        let Phase::ShowKey(mut shown) = std::mem::replace(&mut self.phase, Phase::Form) else {
            return;
        };
        shown.error = None;
        let keyring = shown.keyring.clone();
        let options = self.options.clone();
        let (tx, rx) = channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            // The key file and the config come first; the service only
            // matters once they exist. A service failure is not fatal.
            let result = setup::install(&keyring)
                .map(|()| setup::enable_agent(&options).err())
                .map_err(|e| e.to_string());
            let _ = tx.send(result);
            ctx.request_repaint();
        });
        self.phase = Phase::Installing(shown, rx);
    }

    fn done_ui(&mut self, ui: &mut egui::Ui) -> SetupStatus {
        let Phase::Done { agent_error, retry } = &mut self.phase else {
            return SetupStatus::Running;
        };
        if retry.is_some() {
            busy(ui, "Starting the capture service...");
            return SetupStatus::Running;
        }
        let mut retry_now = false;
        match agent_error {
            None => {
                ui.label("TimeReel is set up and has started taking screenshots.");
            }
            Some(error) => {
                ui.label("Your key is saved, but the capture service could not be started:");
                ui.colored_label(ERROR_COLOR, error.as_str());
                ui.label("No screenshots are taken until it runs. You can start it yourself with:");
                ui.add(
                    egui::Label::new(
                        RichText::new("systemctl --user enable --now timereel-agent.service")
                            .monospace(),
                    )
                    .selectable(true),
                );
                retry_now = ui.button("Try again").clicked();
            }
        }
        ui.add_space(8.0);
        let finished = ui.button(CONTINUE_LABEL).clicked();
        if retry_now {
            let options = self.options.clone();
            let (tx, rx) = channel();
            let ctx = ui.ctx().clone();
            std::thread::spawn(move || {
                let _ = tx.send(setup::enable_agent(&options));
                ctx.request_repaint();
            });
            if let Phase::Done { retry, .. } = &mut self.phase {
                *retry = Some(rx);
            }
        }
        if finished {
            SetupStatus::Finished
        } else {
            SetupStatus::Running
        }
    }
}

enum Poll<T> {
    Waiting,
    Ready(Result<T, String>),
}

fn poll_worker<T>(rx: &Worker<T>, ctx: &egui::Context) -> Poll<T> {
    match rx.try_recv() {
        Ok(result) => Poll::Ready(result),
        Err(TryRecvError::Empty) => {
            ctx.request_repaint_after(Duration::from_millis(100));
            Poll::Waiting
        }
        Err(TryRecvError::Disconnected) => {
            Poll::Ready(Err("Set-up stopped unexpectedly. Please try again.".into()))
        }
    }
}

fn busy(ui: &mut egui::Ui, message: &str) {
    ui.horizontal(|ui| {
        ui.spinner();
        ui.label(message);
    });
}

/// Splits the grouped recovery key (`trk1-abcd-efgh-...`) into lines of a
/// few groups each, so it is easy to read and copy by hand.
fn recovery_lines(text: &str) -> Vec<String> {
    let groups: Vec<&str> = text.split('-').collect();
    groups
        .chunks(GROUPS_PER_LINE)
        .map(|chunk| chunk.join(" "))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::recovery_lines;

    #[test]
    fn lines_hold_four_groups() {
        let lines = recovery_lines("trk1-aaaa-bbbb-cccc-dddd-eeee");
        assert_eq!(lines, ["trk1 aaaa bbbb cccc", "dddd eeee"]);
    }
}
