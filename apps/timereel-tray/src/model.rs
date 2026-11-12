//! The pure part of the tray: what the agent state looks like to the user.
//!
//! Nothing here touches D-Bus or the screen. [`view`] maps an
//! [`AgentSnapshot`] to a [`TrayView`] (icon, tooltip and menu items), and
//! [`icon_pixmap`] draws the icon. Both are plain functions so that they can be
//! tested without a bus.

/// The reason shown when the agent is not on the bus.
pub const AGENT_NOT_RUNNING: &str = "Agent not running";

/// The agent state as published in the `State` property.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    /// Captures are being taken.
    Recording,
    /// The user paused recording.
    Paused,
    /// The last capture failed.
    Error,
    /// Screen capture permission is missing.
    NeedsPermission,
    /// There is no public key yet.
    NotSetUp,
}

impl State {
    /// Parses the D-Bus string. An unknown string counts as [`State::Error`],
    /// so a newer agent never makes the tray look healthy by mistake.
    pub fn parse(text: &str) -> State {
        match text {
            "recording" => State::Recording,
            "paused" => State::Paused,
            "needs_permission" => State::NeedsPermission,
            "not_set_up" => State::NotSetUp,
            _ => State::Error,
        }
    }
}

/// What the tray knows about the agent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AgentSnapshot {
    /// The agent owns its bus name.
    Running {
        /// The `State` property.
        state: State,
        /// The `Reason` property, possibly empty.
        reason: String,
    },
    /// The agent is not on the bus.
    NotRunning,
}

/// Which of the three icons to show.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IconKind {
    /// Capturing normally.
    Recording,
    /// Paused by the user.
    Paused,
    /// Anything that needs the user's attention (also `needs_permission`).
    Error,
}

/// What a menu item does when it is clicked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuAction {
    /// Call `Pause` on the agent.
    Pause,
    /// Call `Resume` on the agent.
    Resume,
    /// Activate the viewer, or start it.
    OpenViewer,
    /// Call `RetryPermission` on the agent.
    GrantPermission,
    /// Ask the viewer to quit and exit the tray.
    Quit,
}

/// One entry of the tray menu.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MenuEntry {
    /// The action behind the entry.
    pub action: MenuAction,
    /// The text shown to the user.
    pub label: &'static str,
    /// False if the entry is shown greyed out.
    pub enabled: bool,
}

/// Everything the tray shows for one agent snapshot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrayView {
    /// The icon to show.
    pub icon: IconKind,
    /// The first line of the tooltip.
    pub tooltip_title: String,
    /// The second line of the tooltip: the reason, empty if there is none.
    pub tooltip_text: String,
    /// The menu, top to bottom.
    pub menu: Vec<MenuEntry>,
}

/// The icon for a snapshot. `needs_permission`, `not_set_up` and a missing
/// agent all use the error icon.
pub fn icon_for(snapshot: &AgentSnapshot) -> IconKind {
    match snapshot {
        AgentSnapshot::Running {
            state: State::Recording,
            ..
        } => IconKind::Recording,
        AgentSnapshot::Running {
            state: State::Paused,
            ..
        } => IconKind::Paused,
        _ => IconKind::Error,
    }
}

fn state_title(snapshot: &AgentSnapshot) -> &'static str {
    match snapshot {
        AgentSnapshot::Running { state, .. } => match state {
            State::Recording => "TimeReel: recording",
            State::Paused => "TimeReel: paused",
            State::Error => "TimeReel: error",
            State::NeedsPermission => "TimeReel: permission needed",
            State::NotSetUp => "TimeReel: not set up",
        },
        AgentSnapshot::NotRunning => "TimeReel: error",
    }
}

/// The reason text of a snapshot. If the agent gives no reason in an error
/// state, a generic text is used, so the tooltip never says nothing.
pub fn reason_for(snapshot: &AgentSnapshot) -> String {
    match snapshot {
        AgentSnapshot::NotRunning => AGENT_NOT_RUNNING.to_string(),
        AgentSnapshot::Running { state, reason } => {
            if !reason.is_empty() {
                reason.clone()
            } else {
                match state {
                    State::Error => "Capture failed".to_string(),
                    State::NeedsPermission => "Screen capture permission needed".to_string(),
                    State::NotSetUp => "Open TimeReel to finish the set-up".to_string(),
                    State::Recording | State::Paused => String::new(),
                }
            }
        }
    }
}

/// The menu for a snapshot. Pause/Resume follows the state and is greyed out
/// when there is nothing to call (no agent, or no set-up yet). Grant capture
/// permission appears only for `needs_permission`.
pub fn menu_for(snapshot: &AgentSnapshot) -> Vec<MenuEntry> {
    let (toggle, toggle_enabled) = match snapshot {
        AgentSnapshot::Running {
            state: State::Paused,
            ..
        } => (MenuAction::Resume, true),
        AgentSnapshot::Running {
            state: State::NotSetUp,
            ..
        }
        | AgentSnapshot::NotRunning => (MenuAction::Pause, false),
        AgentSnapshot::Running { .. } => (MenuAction::Pause, true),
    };
    let mut menu = vec![MenuEntry {
        action: toggle,
        label: if toggle == MenuAction::Resume {
            "Resume"
        } else {
            "Pause"
        },
        enabled: toggle_enabled,
    }];
    menu.push(MenuEntry {
        action: MenuAction::OpenViewer,
        label: "Open TimeReel",
        enabled: true,
    });
    if matches!(
        snapshot,
        AgentSnapshot::Running {
            state: State::NeedsPermission,
            ..
        }
    ) {
        menu.push(MenuEntry {
            action: MenuAction::GrantPermission,
            label: "Grant capture permission",
            enabled: true,
        });
    }
    menu.push(MenuEntry {
        action: MenuAction::Quit,
        label: "Quit",
        enabled: true,
    });
    menu
}

/// Maps a snapshot to everything the tray shows.
pub fn view(snapshot: &AgentSnapshot) -> TrayView {
    TrayView {
        icon: icon_for(snapshot),
        tooltip_title: state_title(snapshot).to_string(),
        tooltip_text: reason_for(snapshot),
        menu: menu_for(snapshot),
    }
}

/// Draws an icon as a `size` x `size` ARGB32 image in network byte order, the
/// format of the StatusNotifierItem `IconPixmap`: a coloured disc, with two
/// bars for paused and an exclamation mark for error. Drawing it here means
/// the tray needs no icon files and no icon theme.
pub fn icon_pixmap(kind: IconKind, size: u32) -> Vec<u8> {
    let (disc, mark): ([u8; 3], [u8; 3]) = match kind {
        IconKind::Recording => ([0x2e, 0xa0, 0x43], [0xff, 0xff, 0xff]),
        IconKind::Paused => ([0x8a, 0x8f, 0x98], [0xff, 0xff, 0xff]),
        IconKind::Error => ([0xd7, 0x3a, 0x3a], [0xff, 0xff, 0xff]),
    };
    let s = size as f32;
    let centre = s / 2.0;
    let radius = s * 0.46;
    let mut data = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            // Coordinates of the pixel centre, in the range 0..1.
            let fx = (x as f32 + 0.5) / s;
            let fy = (y as f32 + 0.5) / s;
            let dx = x as f32 + 0.5 - centre;
            let dy = y as f32 + 0.5 - centre;
            let dist = (dx * dx + dy * dy).sqrt();
            // One pixel of soft edge.
            let alpha = (radius - dist + 0.5).clamp(0.0, 1.0);
            let on_mark = match kind {
                IconKind::Recording => dist < s * 0.17,
                IconKind::Paused => {
                    (0.30..0.70).contains(&fy)
                        && ((0.31..0.45).contains(&fx) || (0.55..0.69).contains(&fx))
                }
                IconKind::Error => {
                    (0.46..0.54).contains(&fx)
                        && ((0.24..0.60).contains(&fy)
                            || (dx * dx + (fy * s - s * 0.72).powi(2)).sqrt() < s * 0.06)
                }
            };
            let rgb = if on_mark { mark } else { disc };
            data.extend_from_slice(&[(alpha * 255.0).round() as u8, rgb[0], rgb[1], rgb[2]]);
        }
    }
    data
}

#[cfg(test)]
mod tests {
    use super::*;

    fn running(state: State, reason: &str) -> AgentSnapshot {
        AgentSnapshot::Running {
            state,
            reason: reason.to_string(),
        }
    }

    fn labels(snapshot: &AgentSnapshot) -> Vec<&'static str> {
        menu_for(snapshot).iter().map(|e| e.label).collect()
    }

    #[test]
    fn parse_known_and_unknown_states() {
        assert_eq!(State::parse("recording"), State::Recording);
        assert_eq!(State::parse("paused"), State::Paused);
        assert_eq!(State::parse("error"), State::Error);
        assert_eq!(State::parse("needs_permission"), State::NeedsPermission);
        assert_eq!(State::parse("not_set_up"), State::NotSetUp);
        assert_eq!(State::parse("from-the-future"), State::Error);
    }

    #[test]
    fn icon_per_state() {
        assert_eq!(
            icon_for(&running(State::Recording, "")),
            IconKind::Recording
        );
        assert_eq!(icon_for(&running(State::Paused, "")), IconKind::Paused);
        assert_eq!(icon_for(&running(State::Error, "x")), IconKind::Error);
        // needs_permission uses the error icon.
        assert_eq!(
            icon_for(&running(State::NeedsPermission, "")),
            IconKind::Error
        );
        assert_eq!(icon_for(&running(State::NotSetUp, "")), IconKind::Error);
        assert_eq!(icon_for(&AgentSnapshot::NotRunning), IconKind::Error);
    }

    #[test]
    fn tooltip_shows_the_reason() {
        let v = view(&running(State::Error, "Disk full"));
        assert_eq!(v.tooltip_title, "TimeReel: error");
        assert_eq!(v.tooltip_text, "Disk full");
        let v = view(&running(
            State::NeedsPermission,
            "Screen capture permission denied",
        ));
        assert_eq!(v.tooltip_title, "TimeReel: permission needed");
        assert_eq!(v.tooltip_text, "Screen capture permission denied");
    }

    #[test]
    fn tooltip_falls_back_when_the_reason_is_empty() {
        assert_eq!(reason_for(&running(State::Error, "")), "Capture failed");
        assert_eq!(
            reason_for(&running(State::NeedsPermission, "")),
            "Screen capture permission needed"
        );
        assert_eq!(reason_for(&running(State::Recording, "")), "");
        assert_eq!(reason_for(&running(State::Paused, "")), "");
    }

    #[test]
    fn tooltip_when_the_agent_is_gone() {
        let v = view(&AgentSnapshot::NotRunning);
        assert_eq!(v.icon, IconKind::Error);
        assert_eq!(v.tooltip_title, "TimeReel: error");
        assert_eq!(v.tooltip_text, "Agent not running");
    }

    #[test]
    fn menu_while_recording_offers_pause() {
        assert_eq!(
            labels(&running(State::Recording, "")),
            ["Pause", "Open TimeReel", "Quit"]
        );
        assert_eq!(
            menu_for(&running(State::Recording, ""))[0].action,
            MenuAction::Pause
        );
    }

    #[test]
    fn menu_while_paused_offers_resume() {
        let menu = menu_for(&running(State::Paused, ""));
        assert_eq!(menu[0].label, "Resume");
        assert_eq!(menu[0].action, MenuAction::Resume);
        assert!(menu[0].enabled);
        assert_eq!(labels(&running(State::Paused, "")).len(), 3);
    }

    #[test]
    fn grant_permission_only_when_needed() {
        let menu = menu_for(&running(State::NeedsPermission, "denied"));
        let grant = menu
            .iter()
            .find(|e| e.action == MenuAction::GrantPermission)
            .expect("grant entry");
        assert_eq!(grant.label, "Grant capture permission");
        assert!(grant.enabled);
        for s in [
            running(State::Recording, ""),
            running(State::Paused, ""),
            running(State::Error, "x"),
            running(State::NotSetUp, ""),
            AgentSnapshot::NotRunning,
        ] {
            assert!(
                menu_for(&s)
                    .iter()
                    .all(|e| e.action != MenuAction::GrantPermission),
                "{s:?}"
            );
        }
    }

    #[test]
    fn pause_is_greyed_out_without_an_agent_or_set_up() {
        for s in [running(State::NotSetUp, ""), AgentSnapshot::NotRunning] {
            let menu = menu_for(&s);
            assert_eq!(menu[0].action, MenuAction::Pause);
            assert!(!menu[0].enabled, "{s:?}");
        }
        assert!(menu_for(&running(State::Error, "x"))[0].enabled);
    }

    #[test]
    fn open_and_quit_are_always_there_and_enabled() {
        for s in [
            running(State::Recording, ""),
            running(State::Paused, ""),
            running(State::Error, "x"),
            running(State::NeedsPermission, ""),
            running(State::NotSetUp, ""),
            AgentSnapshot::NotRunning,
        ] {
            let menu = menu_for(&s);
            assert_eq!(menu[menu.len() - 1].action, MenuAction::Quit, "{s:?}");
            assert!(menu
                .iter()
                .any(|e| e.action == MenuAction::OpenViewer && e.enabled));
            assert!(menu.iter().all(|e| e.action == MenuAction::Pause
                || e.action == MenuAction::Resume
                || e.enabled));
        }
    }

    #[test]
    fn pixmaps_have_the_right_size_and_differ() {
        let a = icon_pixmap(IconKind::Recording, 22);
        let b = icon_pixmap(IconKind::Paused, 22);
        let c = icon_pixmap(IconKind::Error, 22);
        assert_eq!(a.len(), 22 * 22 * 4);
        assert_ne!(a, b);
        assert_ne!(b, c);
        assert_ne!(a, c);
        // The corner is transparent, the middle of the disc is not.
        assert_eq!(a[0], 0);
        let mid = (11 * 22 + 11) * 4;
        assert_eq!(a[mid], 255);
    }
}
