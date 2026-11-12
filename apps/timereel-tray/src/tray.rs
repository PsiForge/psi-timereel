//! The StatusNotifierItem: a thin `ksni::Tray` over [`TrayView`].

use std::sync::Arc;

use ksni::menu::{MenuItem, StandardItem};
use ksni::{Icon, ToolTip, Tray};
use tokio::runtime::Handle;
use tokio::sync::Notify;

use crate::bus::Actions;
use crate::model::{icon_pixmap, view, AgentSnapshot, IconKind, MenuAction, TrayView};

/// Icon sizes offered to the host; it picks the closest.
const ICON_SIZES: [u32; 3] = [22, 32, 48];

/// The tray model that ksni reads. It holds the current [`TrayView`] and what
/// is needed to act on a click.
pub struct TimeReelTray {
    view: TrayView,
    actions: Actions,
    runtime: Handle,
    quit: Arc<Notify>,
}

impl TimeReelTray {
    /// Makes a tray that starts in the "agent not running" state. Must be
    /// called inside a tokio runtime. `quit` is notified when the user chose
    /// Quit and the viewer has been asked to quit.
    pub fn new(actions: Actions, quit: Arc<Notify>) -> TimeReelTray {
        TimeReelTray {
            view: view(&AgentSnapshot::NotRunning),
            actions,
            runtime: Handle::current(),
            quit,
        }
    }

    /// Shows a new snapshot.
    pub fn set_snapshot(&mut self, snapshot: &AgentSnapshot) {
        self.view = view(snapshot);
    }

    /// What is shown now.
    pub fn view(&self) -> &TrayView {
        &self.view
    }

    /// Runs the action of a clicked entry. Menu callbacks must not block, so
    /// the call runs as a task.
    fn run(&self, action: MenuAction) {
        let actions = self.actions.clone();
        let quit = Arc::clone(&self.quit);
        self.runtime.spawn(async move {
            let result = match action {
                MenuAction::Pause => actions.pause().await.map_err(|e| e.to_string()),
                MenuAction::Resume => actions.resume().await.map_err(|e| e.to_string()),
                MenuAction::GrantPermission => {
                    actions.retry_permission().await.map_err(|e| e.to_string())
                }
                MenuAction::OpenViewer => actions.open_viewer().await,
                MenuAction::Quit => {
                    let result = actions.quit_viewer().await;
                    // Exit even if the viewer did not answer.
                    quit.notify_one();
                    result
                }
            };
            if let Err(e) = result {
                eprintln!("timereel-tray: {action:?} failed: {e}");
            }
        });
    }
}

impl Tray for TimeReelTray {
    fn id(&self) -> String {
        "timereel-tray".into()
    }

    fn title(&self) -> String {
        "TimeReel".into()
    }

    fn icon_pixmap(&self) -> Vec<Icon> {
        pixmaps(self.view.icon)
    }

    fn tool_tip(&self) -> ToolTip {
        ToolTip {
            icon_name: String::new(),
            icon_pixmap: pixmaps(self.view.icon),
            title: self.view.tooltip_title.clone(),
            description: self.view.tooltip_text.clone(),
        }
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        self.run(MenuAction::OpenViewer);
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        self.view
            .menu
            .iter()
            .map(|entry| {
                let action = entry.action;
                StandardItem {
                    label: entry.label.to_string(),
                    enabled: entry.enabled,
                    activate: Box::new(move |this: &mut TimeReelTray| this.run(action)),
                    ..Default::default()
                }
                .into()
            })
            .collect()
    }
}

fn pixmaps(kind: IconKind) -> Vec<Icon> {
    ICON_SIZES
        .iter()
        .map(|&size| Icon {
            width: size as i32,
            height: size as i32,
            data: icon_pixmap(kind, size),
        })
        .collect()
}
