//! egui_kittest tests of the Settings screen: validation, the retention
//! confirmation, the excluded apps, pause, and Change password.

mod settings_support;

use egui_kittest::kittest::Queryable;
use settings_support::*;
use timereel::lock_ui::WRONG_SECRET_MESSAGE;
use timereel::settings::*;
use timereel_config::Config;
use timereel_store::CaptureTime;

const NOW: CaptureTime = CaptureTime(100 * DAY_MS);
const PASSWORD: &str = "correct horse battery";

#[test]
fn invalid_values_show_the_field_message_and_save_is_refused() {
    let env = TestEnv::new();
    let agent = FakeAgent::running();
    let mut h = settings_harness(&env, Box::new(agent.clone()), NOW);

    set_field(&mut h, 0, "0");
    assert!(has_text(
        &h,
        "The capture interval must be between 1 and 60 minutes, but is 0"
    ));
    set_field(&mut h, 2, "abc");
    assert!(has_text(&h, "The image quality must be a whole number"));
    set_field(&mut h, 5, "1000");
    assert!(has_text(
        &h,
        "The auto-lock time must be between 1 and 240 minutes, but is 1000"
    ));

    h.get_by_label(SAVE_LABEL).click();
    h.run_steps(2);
    assert!(has_text(&h, REFUSED_MESSAGE));
    assert!(!env.config_file().exists(), "nothing was written");
    assert!(agent.calls.borrow().is_empty(), "the agent was not called");

    // Fixing the values clears the messages and allows the save.
    set_field(&mut h, 0, "7");
    set_field(&mut h, 2, "60");
    set_field(&mut h, 5, "20");
    assert!(!has_text(&h, "must be"));
    h.get_by_label(SAVE_LABEL).click();
    h.run_steps(2);
    let config = read_config();
    assert_eq!(
        (
            config.interval_minutes,
            config.quality,
            config.ui_autolock_minutes
        ),
        (7, 60, 20)
    );
}

#[test]
fn an_invalid_excluded_app_is_refused_with_the_config_message() {
    let env = TestEnv::new();
    let mut h = settings_harness(&env, Box::new(FakeAgent::running()), NOW);
    h.get_by_label(ADD_APP_LABEL).click();
    h.run_steps(2);
    assert!(has_text(&h, "An excluded app name must not be empty"));
    h.get_by_label(SAVE_LABEL).click();
    h.run_steps(2);
    assert!(!env.config_file().exists());
    assert!(has_text(&h, REFUSED_MESSAGE));
}

#[test]
fn the_form_edits_excluded_apps_and_the_unknown_toggle() {
    let env = TestEnv::new();
    let mut h = settings_harness(&env, Box::new(FakeAgent::running()), NOW);
    assert!(has_text(&h, "KeePassXC, 1Password, Bitwarden"));
    assert!(has_text(&h, "Privacy:"));
    h.get_by_label(ADD_APP_LABEL).click();
    h.run_steps(2);
    set_field(&mut h, 6, "KeePassXC");
    h.get_by_label(ADD_APP_LABEL).click();
    h.run_steps(2);
    set_field(&mut h, 7, "Bitwarden");
    h.get_by_label(CAPTURE_UNKNOWN_LABEL).click();
    h.run_steps(2);
    h.get_by_label(SAVE_LABEL).click();
    h.run_steps(2);
    let config = read_config();
    assert_eq!(config.excluded_apps, ["KeePassXC", "Bitwarden"]);
    assert!(config.capture_when_app_unknown);

    // Removing the first row and saving again.
    h.get_all_by_label(REMOVE_APP_LABEL)
        .next()
        .expect("remove button")
        .click();
    h.run_steps(2);
    h.get_by_label(SAVE_LABEL).click();
    h.run_steps(2);
    assert_eq!(read_config().excluded_apps, ["Bitwarden"]);
}

#[test]
fn with_the_agent_missing_save_still_writes_the_config_and_says_so() {
    let env = TestEnv::new();
    let agent = FakeAgent::missing();
    let mut h = settings_harness(&env, Box::new(agent.clone()), NOW);
    set_field(&mut h, 0, "9");
    h.get_by_label(SAVE_LABEL).click();
    h.run_steps(2);
    assert_eq!(read_config().interval_minutes, 9);
    assert!(has_text(&h, SAVED_OFFLINE_MESSAGE));
    assert!(!has_text(&h, SAVED_MESSAGE));
    assert_eq!(*agent.calls.borrow(), ["ReloadConfig"]);
}

#[test]
fn a_running_agent_gets_reload_config_after_the_save() {
    let env = TestEnv::new();
    let agent = FakeAgent::running();
    let mut h = settings_harness(&env, Box::new(agent.clone()), NOW);
    set_field(&mut h, 1, "1600");
    h.get_by_label(SAVE_LABEL).click();
    h.run_steps(2);
    assert_eq!(read_config().width_px, 1600);
    assert_eq!(*agent.calls.borrow(), ["ReloadConfig"]);
    assert!(has_text(&h, SAVED_MESSAGE));
}

#[test]
fn saving_keeps_paused_and_the_public_key_that_changed_meanwhile() {
    let env = TestEnv::new();
    let mut h = settings_harness(&env, Box::new(FakeAgent::running()), NOW);
    timereel_config::update(|c| {
        c.paused = true;
        c.public_key = Some("age1example".into());
    })
    .expect("update");
    set_field(&mut h, 0, "8");
    h.get_by_label(SAVE_LABEL).click();
    h.run_steps(2);
    let config = read_config();
    assert_eq!(config.interval_minutes, 8);
    assert!(config.paused);
    assert_eq!(config.public_key.as_deref(), Some("age1example"));
}

#[test]
fn the_pause_toggle_calls_the_agent_and_reads_the_state_back() {
    let env = TestEnv::new();
    let agent = FakeAgent::running();
    let mut h = settings_harness(&env, Box::new(agent.clone()), NOW);
    h.get_by_label(PAUSE_LABEL).click();
    h.run_steps(2);
    assert_eq!(*agent.calls.borrow(), ["Pause"]);
    assert!(read_config().paused);
    h.get_by_label(PAUSE_LABEL).click();
    h.run_steps(2);
    assert_eq!(*agent.calls.borrow(), ["Pause", "Resume"]);
    assert!(!read_config().paused);
}

#[test]
fn the_pause_toggle_says_so_when_the_agent_is_missing() {
    let env = TestEnv::new();
    let mut h = settings_harness(&env, Box::new(FakeAgent::missing()), NOW);
    h.get_by_label(PAUSE_LABEL).click();
    h.run_steps(2);
    assert!(has_text(&h, PAUSE_OFFLINE_MESSAGE));
    assert!(!read_config().paused);
}

fn add_old_captures(env: &TestEnv) -> Vec<std::path::PathBuf> {
    // Ages in days: 5 and 15 and 20 and 40. Retention is 30 days at first.
    [5, 15, 20, 40]
        .into_iter()
        .map(|d| env.add_capture(CaptureTime(NOW.0 - d * DAY_MS)))
        .collect()
}

#[test]
fn lowering_retention_confirms_with_the_count_and_cancel_deletes_nothing() {
    let env = TestEnv::new();
    let files = add_old_captures(&env);
    let agent = FakeAgent::running();
    let mut h = settings_harness(&env, Box::new(agent.clone()), NOW);

    // 10 days: the captures that are 15, 20 and 40 days old would go.
    set_field(&mut h, 4, "10");
    h.get_by_label(SAVE_LABEL).click();
    h.run_steps(2);
    assert!(has_text(
        &h,
        "Keeping captures for 10 days will delete 3 captures"
    ));
    assert!(
        !env.config_file().exists(),
        "nothing is saved before the OK"
    );

    h.get_by_label(CANCEL_LABEL).click();
    h.run_steps(2);
    assert!(files.iter().all(|f| f.exists()), "nothing was deleted");
    assert!(agent.calls.borrow().is_empty());
    assert!(!env.config_file().exists());
    // The form shows the old value again.
    assert!(!has_text(&h, "will delete"));
    assert_eq!(read_config().retention_days, 30);
    h.get_by_label(SAVE_LABEL).click();
    h.run_steps(2);
    assert_eq!(
        read_config().retention_days,
        Config::default().retention_days,
        "a plain save after Cancel keeps the old value"
    );
    assert!(!has_text(&h, "will delete"));
}

#[test]
fn confirming_lower_retention_saves_reloads_and_runs_retention() {
    let env = TestEnv::new();
    add_old_captures(&env);
    let mut agent = FakeAgent::running();
    agent.deleted = 3;
    let mut h = settings_harness(&env, Box::new(agent.clone()), NOW);
    set_field(&mut h, 4, "10");
    h.get_by_label(SAVE_LABEL).click();
    h.run_steps(2);
    h.get_by_label(CONFIRM_DELETE_LABEL).click();
    h.run_steps(2);
    assert_eq!(read_config().retention_days, 10);
    assert_eq!(*agent.calls.borrow(), ["ReloadConfig", "RunRetention"]);
    assert!(has_text(&h, "3 captures were deleted"));
}

#[test]
fn raising_retention_needs_no_confirmation() {
    let env = TestEnv::new();
    add_old_captures(&env);
    let agent = FakeAgent::running();
    let mut h = settings_harness(&env, Box::new(agent.clone()), NOW);
    set_field(&mut h, 4, "60");
    h.get_by_label(SAVE_LABEL).click();
    h.run_steps(2);
    assert_eq!(read_config().retention_days, 60);
    assert_eq!(*agent.calls.borrow(), ["ReloadConfig"]);
}

#[test]
fn confirming_with_the_agent_missing_saves_and_says_when_it_applies() {
    let env = TestEnv::new();
    add_old_captures(&env);
    let agent = FakeAgent::missing();
    let mut h = settings_harness(&env, Box::new(agent.clone()), NOW);
    set_field(&mut h, 4, "10");
    h.get_by_label(SAVE_LABEL).click();
    h.run_steps(2);
    h.get_by_label(CONFIRM_DELETE_LABEL).click();
    h.run_steps(2);
    assert_eq!(read_config().retention_days, 10);
    assert!(has_text(&h, SAVED_OFFLINE_MESSAGE));
    assert_eq!(env.count_captures(), 4, "the agent deletes at its start");
}

#[test]
fn change_password_with_mismatched_new_passwords_shows_a_message() {
    let env = TestEnv::new();
    env.make_keyring(PASSWORD);
    let mut h = settings_harness(&env, Box::new(FakeAgent::running()), NOW);
    h.get_by_label(CHANGE_PASSWORD_LABEL).click();
    h.run_steps(2);
    type_password(&mut h, 0, PASSWORD);
    type_password(&mut h, 1, "a brand new passphrase");
    type_password(&mut h, 2, "another new passphrase");
    assert!(has_text(&h, "The two passwords do not match."));
    // The button is disabled: the key file still has the old password.
    h.get_by_label(CHANGE_PASSWORD_SUBMIT_LABEL).click();
    h.run_steps(3);
    assert!(has_text(&h, "The two passwords do not match."));
    assert!(!has_text(&h, PASSWORD_CHANGED_MESSAGE));
    assert!(unlocks(&env, PASSWORD));
}

fn unlocks(env: &TestEnv, password: &str) -> bool {
    use timereel_crypto::{load, unlock_with_password, SecretString};
    let keyring = load(&env.keyring_path()).expect("keyring");
    unlock_with_password(&keyring, &SecretString::from(password.to_owned())).is_ok()
}

#[test]
fn change_password_with_the_current_password_makes_the_new_one_unlock() {
    let env = TestEnv::new();
    env.make_keyring(PASSWORD);
    let mut h = settings_harness(&env, Box::new(FakeAgent::running()), NOW);
    h.get_by_label(CHANGE_PASSWORD_LABEL).click();
    h.run_steps(2);
    let new = "a brand new passphrase";
    type_password(&mut h, 0, PASSWORD);
    type_password(&mut h, 1, new);
    type_password(&mut h, 2, new);
    h.get_by_label(CHANGE_PASSWORD_SUBMIT_LABEL).click();
    wait_until(&mut h, "password changed", |h| {
        has_text(h, PASSWORD_CHANGED_MESSAGE)
    });
    assert!(unlocks(&env, new));
    assert!(!unlocks(&env, PASSWORD));
}

#[test]
fn change_password_with_the_recovery_key_works_and_a_wrong_secret_is_refused() {
    let env = TestEnv::new();
    let recovery = env.make_keyring(PASSWORD);
    let mut h = settings_harness(&env, Box::new(FakeAgent::running()), NOW);
    h.get_by_label(CHANGE_PASSWORD_LABEL).click();
    h.run_steps(2);
    let new = "a brand new passphrase";
    type_password(&mut h, 0, "not the password");
    type_password(&mut h, 1, new);
    type_password(&mut h, 2, new);
    h.get_by_label(CHANGE_PASSWORD_SUBMIT_LABEL).click();
    wait_until(&mut h, "refusal", |h| has_text(h, WRONG_SECRET_MESSAGE));
    assert!(unlocks(&env, PASSWORD), "nothing changed");

    // The fields were cleared by the attempt; try again with the recovery key.
    h.get_by_label(USE_RECOVERY_LABEL).click();
    h.run_steps(2);
    type_password(&mut h, 0, &recovery);
    type_password(&mut h, 1, new);
    type_password(&mut h, 2, new);
    h.get_by_label(CHANGE_PASSWORD_SUBMIT_LABEL).click();
    wait_until(&mut h, "password changed", |h| {
        has_text(h, PASSWORD_CHANGED_MESSAGE)
    });
    assert!(unlocks(&env, new));
}

#[test]
fn cancel_closes_the_password_dialog() {
    let env = TestEnv::new();
    env.make_keyring(PASSWORD);
    let mut h = settings_harness(&env, Box::new(FakeAgent::running()), NOW);
    h.get_by_label(CHANGE_PASSWORD_LABEL).click();
    h.run_steps(2);
    h.get_by_label(CANCEL_LABEL).click();
    h.run_steps(2);
    assert!(has_text(&h, SAVE_LABEL));
}
