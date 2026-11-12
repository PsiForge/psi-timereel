//! The TimeReel tray (architecture section 2.3, ADR 0003).
//!
//! - [`model`]: pure functions from the agent state to icon, tooltip and menu.
//! - [`bus`]: following the agent over D-Bus and the calls behind the menu.
//! - [`tray`]: the ksni StatusNotifierItem built on the two.

pub mod bus;
pub mod model;
pub mod tray;

#[cfg(test)]
mod tests {
    /// The autostart entry, as installed to `/etc/xdg/autostart`.
    const DESKTOP: &str = include_str!("../../../packaging/timereel-tray.desktop");

    fn value(key: &str) -> Option<&'static str> {
        DESKTOP
            .lines()
            .find_map(|l| l.strip_prefix(key)?.strip_prefix('='))
    }

    #[test]
    fn autostart_entry_starts_the_tray() {
        assert!(DESKTOP.starts_with("[Desktop Entry]\n"));
        assert_eq!(value("Type"), Some("Application"));
        assert_eq!(value("Exec"), Some("timereel-tray"));
        assert!(value("Name").is_some());
        // The tray has no window and must not appear in the app menu.
        assert_eq!(value("NoDisplay"), Some("true"));
    }
}
