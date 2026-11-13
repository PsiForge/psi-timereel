//! The decrypted metadata of every capture, loaded in the background.
//!
//! After unlock the viewer calls [`MetaStore::queue`] with the capture times
//! (newest first is best, as the newest are on screen first). One worker
//! thread reads and decrypts each capture's metadata through a
//! [`MetaSource`] (about 1 ms each) and calls the repaint callback every
//! [`REPAINT_EVERY`] entries and at the end of a batch.
//!
//! The cache is meant to be held by the viewer's `Session`: the closure from
//! [`MetaStore::lock_hook`] goes into `Session::on_lock`; it empties the map,
//! stops the worker and, with it, drops the source and its key. The cache
//! writes nothing to disk.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;

use age::x25519::Identity;
use timereel_store::{CaptureMeta, CaptureTime, Store};

/// The worker asks for a repaint after this many loaded entries.
pub const REPAINT_EVERY: usize = 64;

/// Reads and decrypts the metadata of one capture. It runs on the worker.
pub trait MetaSource: Send + Sync + 'static {
    /// The metadata, or a message for the user. The message must not contain
    /// app names or window titles.
    fn load(&self, time: CaptureTime) -> Result<CaptureMeta, String>;
}

/// The real source: [`Store::read_meta`] with the private key.
pub struct StoreMetaSource {
    store: Store,
    identity: Identity,
}

impl StoreMetaSource {
    /// A source that opens captures of `store` with `identity`.
    pub fn new(store: Store, identity: Identity) -> Self {
        Self { store, identity }
    }
}

impl MetaSource for StoreMetaSource {
    fn load(&self, time: CaptureTime) -> Result<CaptureMeta, String> {
        self.store
            .read_meta(&self.identity, time)
            .map_err(|e| e.to_string())
    }
}

/// What is known about one capture's metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MetaEntry {
    /// Decrypted.
    Ready(CaptureMeta),
    /// Reading failed; the message is safe to show.
    Failed(String),
}

#[derive(Default)]
struct Shared {
    map: Mutex<HashMap<CaptureTime, MetaEntry>>,
    closed: AtomicBool,
    /// Captures handed to [`MetaStore::queue`] so far.
    total: AtomicUsize,
    /// Captures the worker has dealt with (loaded, failed or already known).
    done: AtomicUsize,
}

impl Shared {
    fn map(&self) -> MutexGuard<'_, HashMap<CaptureTime, MetaEntry>> {
        self.map.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
        self.map().clear();
        self.total.store(0, Ordering::SeqCst);
        self.done.store(0, Ordering::SeqCst);
    }
}

/// The metadata of all captures, filled by a background thread.
pub struct MetaStore {
    shared: Arc<Shared>,
    batches: Option<Sender<Vec<CaptureTime>>>,
    worker: Option<JoinHandle<()>>,
}

impl MetaStore {
    /// Starts the worker. `on_repaint` is called from the worker thread.
    pub fn new(source: Arc<dyn MetaSource>, on_repaint: impl Fn() + Send + 'static) -> Self {
        let shared = Arc::new(Shared::default());
        let (tx, rx) = channel::<Vec<CaptureTime>>();
        let worker_shared = Arc::clone(&shared);
        let worker = std::thread::spawn(move || {
            let mut since_repaint = 0usize;
            while let Ok(batch) = rx.recv() {
                for time in batch {
                    if worker_shared.closed.load(Ordering::SeqCst) {
                        return;
                    }
                    if worker_shared.map().contains_key(&time) {
                        worker_shared.done.fetch_add(1, Ordering::SeqCst);
                        continue;
                    }
                    // A panic in a source must not kill the viewer.
                    let entry = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        source.load(time)
                    }))
                    .unwrap_or_else(|_| Err("Reading the details failed unexpectedly".into()));
                    let entry = match entry {
                        Ok(meta) => MetaEntry::Ready(meta),
                        Err(msg) => MetaEntry::Failed(msg),
                    };
                    let mut map = worker_shared.map();
                    if worker_shared.closed.load(Ordering::SeqCst) {
                        return;
                    }
                    map.insert(time, entry);
                    drop(map);
                    worker_shared.done.fetch_add(1, Ordering::SeqCst);
                    since_repaint += 1;
                    if since_repaint >= REPAINT_EVERY {
                        since_repaint = 0;
                        on_repaint();
                    }
                }
                since_repaint = 0;
                on_repaint();
            }
        });
        Self {
            shared,
            batches: Some(tx),
            worker: Some(worker),
        }
    }

    /// Asks the worker to load these captures, in this order. Times that are
    /// already loaded are skipped. Does nothing after a lock.
    pub fn queue(&self, times: Vec<CaptureTime>) {
        if let Some(tx) = &self.batches {
            let count = times.len();
            // Count first, so that `done` never overtakes `total`.
            self.shared.total.fetch_add(count, Ordering::SeqCst);
            // The worker only ends after a lock; then there is nothing to do.
            if tx.send(times).is_err() {
                self.shared.total.fetch_sub(count, Ordering::SeqCst);
            }
        }
    }

    /// How far the background decryption is: `(done, total)` over all
    /// captures queued so far. The UI shows this while `done < total`;
    /// searching works on the part already loaded.
    pub fn progress(&self) -> (usize, usize) {
        // Read `done` first so that it can never exceed `total`.
        let done = self.shared.done.load(Ordering::SeqCst);
        let total = self.shared.total.load(Ordering::SeqCst);
        (done.min(total), total)
    }

    /// Whether every queued capture has been dealt with.
    pub fn is_complete(&self) -> bool {
        let (done, total) = self.progress();
        done >= total
    }

    /// The metadata of `time`, if it has been read.
    pub fn get(&self, time: CaptureTime) -> Option<MetaEntry> {
        self.shared.map().get(&time).cloned()
    }

    /// How many entries are held.
    pub fn len(&self) -> usize {
        self.shared.map().len()
    }

    /// Whether no entry is held.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// A closure for `Session::on_lock`: it empties the cache and stops the
    /// worker (which drops the source and its key).
    pub fn lock_hook(&self) -> impl FnOnce() + Send + 'static {
        let shared = Arc::clone(&self.shared);
        move || shared.close()
    }
}

impl Drop for MetaStore {
    fn drop(&mut self) {
        self.shared.close();
        self.batches = None;
        if let Some(w) = self.worker.take() {
            let _ = w.join();
        }
    }
}
