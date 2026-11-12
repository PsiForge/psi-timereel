//! Single instance through the session bus name `org.timereel.Viewer1`
//! (architecture section 2.4). The first viewer owns the name and serves
//! `Activate` and `Quit`; a second one calls `Activate` on it and exits.

use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};

use zbus::{connection::Builder, Connection, Proxy};

/// The bus name of the viewer.
pub const VIEWER_NAME: &str = "org.timereel.Viewer1";
/// The object path of the viewer.
pub const VIEWER_PATH: &str = "/org/timereel/Viewer1";
/// The interface of the viewer.
pub const VIEWER_INTERFACE: &str = "org.timereel.Viewer1";

/// A request from another process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    /// Bring the window to the front.
    Activate,
    /// Close the viewer.
    Quit,
}

type SharedContext = Arc<Mutex<Option<egui::Context>>>;

struct Viewer1 {
    tx: Mutex<Sender<Command>>,
    ctx: SharedContext,
}

impl Viewer1 {
    fn send(&self, command: Command) {
        if let Ok(tx) = self.tx.lock() {
            // The receiver only goes away when the viewer is closing.
            let _ = tx.send(command);
        }
        if let Ok(ctx) = self.ctx.lock() {
            if let Some(ctx) = ctx.as_ref() {
                ctx.request_repaint();
            }
        }
    }
}

#[zbus::interface(name = "org.timereel.Viewer1")]
impl Viewer1 {
    fn activate(&self) {
        self.send(Command::Activate);
    }

    fn quit(&self) {
        self.send(Command::Quit);
    }
}

/// The first viewer: it owns the bus name for as long as this value lives.
pub struct Primary {
    // Field order matters: the connection is dropped before its runtime.
    _connection: Connection,
    _runtime: tokio::runtime::Runtime,
    commands: Receiver<Command>,
    ctx: SharedContext,
}

impl Primary {
    /// Lets D-Bus calls wake up the UI. Call once the window exists.
    pub fn attach(&self, ctx: &egui::Context) {
        if let Ok(mut slot) = self.ctx.lock() {
            *slot = Some(ctx.clone());
        }
    }

    /// The next request that arrived, if any.
    pub fn try_next(&self) -> Option<Command> {
        self.commands.try_recv().ok()
    }
}

/// Result of [`claim`].
pub enum Claim {
    /// This process is the viewer.
    Primary(Primary),
    /// Another viewer already owns the name.
    AlreadyRunning,
}

/// Tries to become the viewer by taking the bus name.
///
/// # Errors
/// Fails if there is no session bus or it refuses for another reason.
pub fn claim() -> zbus::Result<Claim> {
    let (tx, commands) = channel();
    let ctx: SharedContext = Arc::default();
    let service = Viewer1 {
        tx: Mutex::new(tx),
        ctx: ctx.clone(),
    };
    // The object is served before the name is requested, so a caller that
    // sees the name can always reach it. The name is not queued.
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .thread_name("timereel-dbus")
        .enable_all()
        .build()
        .map_err(zbus::Error::from)?;
    let built = runtime.block_on(async {
        // zbus allows replacing an owner by default; a viewer must never take
        // the name from a running one.
        Builder::session()
            .allow_name_replacements(false)
            .replace_existing_names(false)
            .name(VIEWER_NAME)
            .serve_at(VIEWER_PATH, service)
            .build()
            .await
    });
    match built {
        Ok(connection) => Ok(Claim::Primary(Primary {
            _connection: connection,
            _runtime: runtime,
            commands,
            ctx,
        })),
        Err(zbus::Error::NameTaken) => Ok(Claim::AlreadyRunning),
        Err(e) => Err(e),
    }
}

/// Asks the running viewer to come to the front.
///
/// # Errors
/// Fails if the bus or the other viewer does not answer.
pub fn activate_running() -> zbus::Result<()> {
    call_running("Activate")
}

/// Asks the running viewer to quit.
///
/// # Errors
/// Fails if the bus or the other viewer does not answer.
pub fn quit_running() -> zbus::Result<()> {
    call_running("Quit")
}

fn call_running(method: &str) -> zbus::Result<()> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(zbus::Error::from)?;
    runtime.block_on(async {
        let connection = Connection::session().await?;
        let proxy = Proxy::new(&connection, VIEWER_NAME, VIEWER_PATH, VIEWER_INTERFACE).await?;
        proxy.call::<_, _, ()>(method, &()).await
    })
}
