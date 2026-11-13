//! The background image loader (architecture section 8).
//!
//! The UI thread calls [`ImageLoader::request`], which never blocks: it
//! answers from the cache or queues a job. Worker threads pick the queued job
//! closest to the current position, decrypt and decode it through a
//! [`Decoder`], put the result in a byte-limited cache and send it over a
//! channel, then call the repaint callback. A generation counter makes a
//! jump drop every job that was queued or running before it.
//!
//! The loader writes nothing to disk. Errors become [`Item::Failed`] items.

use std::collections::{HashMap, HashSet};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;

use age::x25519::Identity;
use timereel_image::RgbaImage;
use timereel_store::{CaptureTime, Store};

use crate::index::Index;
use crate::lru::{ByteLru, FULL_CACHE_BYTES, THUMB_CACHE_BYTES};

/// Full images prefetched on each side of the current position.
pub const PREFETCH_FULL: usize = 3;
/// Thumbnails prefetched on each side of the current position.
pub const PREFETCH_THUMBS: usize = 40;
/// Most worker threads.
pub const MAX_WORKERS: usize = 4;

/// Which picture of a capture.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Kind {
    /// The small preview. Cheap, so it goes first on a tie.
    Thumb,
    /// The full image.
    Full,
}

/// Reads, decrypts and decodes one picture. It runs on worker threads.
pub trait Decoder: Send + Sync + 'static {
    /// The decoded picture, or a message for the user. The message must not
    /// contain image data, app names or window titles.
    fn load(&self, kind: Kind, time: CaptureTime) -> Result<RgbaImage, String>;
}

/// The real decoder: [`Store`] read, age decryption and image decode.
pub struct StoreDecoder {
    store: Store,
    identity: Identity,
}

impl StoreDecoder {
    /// A decoder that opens captures of `store` with `identity`.
    pub fn new(store: Store, identity: Identity) -> Self {
        Self { store, identity }
    }
}

impl Decoder for StoreDecoder {
    fn load(&self, kind: Kind, time: CaptureTime) -> Result<RgbaImage, String> {
        let bytes = match kind {
            Kind::Thumb => self.store.read_thumb(&self.identity, time),
            Kind::Full => self.store.read_image(&self.identity, time),
        }
        .map_err(|e| e.to_string())?;
        timereel_image::decode(&bytes).map_err(|e| e.to_string())
    }
}

/// What the loader knows about one picture.
#[derive(Debug, Clone)]
pub enum Item {
    /// Decoded and cached.
    Ready(Arc<RgbaImage>),
    /// Loading failed. The viewer shows this message instead of the picture.
    Failed(String),
}

/// What [`ImageLoader::request`] answers.
#[derive(Debug, Clone)]
pub enum Status {
    /// The picture is cached, or loading failed earlier.
    Done(Item),
    /// A job is queued or running; the result arrives through
    /// [`ImageLoader::poll`].
    Pending,
    /// The loader was locked and does nothing any more.
    Closed,
}

/// A result delivered over the channel.
#[derive(Debug, Clone)]
pub struct Loaded {
    /// The capture.
    pub time: CaptureTime,
    /// Thumbnail or full image.
    pub kind: Kind,
    /// The picture or the error.
    pub item: Item,
}

type Key = (Kind, CaptureTime);

struct State {
    generation: u64,
    position: CaptureTime,
    closed: bool,
    /// Jobs waiting for a worker.
    queue: Vec<Key>,
    /// Everything queued or running in this generation.
    wanted: HashSet<Key>,
    full: ByteLru<CaptureTime, Arc<RgbaImage>>,
    thumbs: ByteLru<CaptureTime, Arc<RgbaImage>>,
    /// Pictures that failed to load, so they are not retried in a loop.
    failed: HashMap<Key, String>,
}

impl State {
    fn cache(&mut self, kind: Kind) -> &mut ByteLru<CaptureTime, Arc<RgbaImage>> {
        match kind {
            Kind::Thumb => &mut self.thumbs,
            Kind::Full => &mut self.full,
        }
    }

    /// Removes and returns the queued job closest to the position.
    fn pop_nearest(&mut self) -> Option<Key> {
        let pos = self.position.0;
        let best = self
            .queue
            .iter()
            .enumerate()
            .min_by_key(|(_, (kind, t))| (t.0.abs_diff(pos), *kind))
            .map(|(i, _)| i)?;
        Some(self.queue.swap_remove(best))
    }

    fn drop_jobs(&mut self) {
        self.generation += 1;
        self.queue.clear();
        self.wanted.clear();
    }
}

struct Shared {
    state: Mutex<State>,
    wake: Condvar,
}

impl Shared {
    /// Locks the state. The lock is only held for short, non-blocking work
    /// (never during a decode), so this does not wait for the workers' jobs.
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|p| p.into_inner())
    }
}

/// The number of worker threads: min(4, CPU count - 1), at least 1.
pub fn default_workers() -> usize {
    let cpus = std::thread::available_parallelism().map_or(2, |n| n.get());
    cpus.saturating_sub(1).clamp(1, MAX_WORKERS)
}

/// A cheap handle to clear the loader. It can be moved into the session's
/// lock hook.
#[derive(Clone)]
pub struct LockHandle(Arc<Shared>);

impl LockHandle {
    /// Empties both caches and the job queue, drops running jobs' results and
    /// stops the workers (which drops the decoder and its key). The loader
    /// is useless afterwards; a new unlock builds a new one.
    pub fn lock(&self) {
        let mut st = self.0.lock();
        st.drop_jobs();
        st.full.clear();
        st.thumbs.clear();
        st.failed.clear();
        st.closed = true;
        drop(st);
        self.0.wake.notify_all();
    }
}

/// The background loader with its caches.
pub struct ImageLoader {
    shared: Arc<Shared>,
    results: Mutex<Receiver<Loaded>>,
    workers: Vec<JoinHandle<()>>,
}

impl ImageLoader {
    /// A loader with [`default_workers`] threads and the standard cache
    /// limits. `on_repaint` is called from a worker after each result.
    pub fn new(decoder: Arc<dyn Decoder>, on_repaint: impl Fn() + Send + Sync + 'static) -> Self {
        Self::with_limits(
            decoder,
            on_repaint,
            default_workers(),
            FULL_CACHE_BYTES,
            THUMB_CACHE_BYTES,
        )
    }

    /// Like [`ImageLoader::new`] with explicit thread count and byte limits.
    pub fn with_limits(
        decoder: Arc<dyn Decoder>,
        on_repaint: impl Fn() + Send + Sync + 'static,
        workers: usize,
        full_bytes: usize,
        thumb_bytes: usize,
    ) -> Self {
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                generation: 0,
                position: CaptureTime(0),
                closed: false,
                queue: Vec::new(),
                wanted: HashSet::new(),
                full: ByteLru::new(full_bytes),
                thumbs: ByteLru::new(thumb_bytes),
                failed: HashMap::new(),
            }),
            wake: Condvar::new(),
        });
        let (tx, rx) = channel();
        let on_repaint: Arc<dyn Fn() + Send + Sync> = Arc::new(on_repaint);
        let workers = (0..workers.max(1))
            .map(|_| {
                let (shared, decoder, tx, repaint) = (
                    Arc::clone(&shared),
                    Arc::clone(&decoder),
                    tx.clone(),
                    Arc::clone(&on_repaint),
                );
                std::thread::spawn(move || worker(&shared, &*decoder, &tx, &*repaint))
            })
            .collect();
        Self {
            shared,
            results: Mutex::new(rx),
            workers,
        }
    }

    /// A handle for the session's lock hook (see [`LockHandle::lock`]).
    pub fn lock_handle(&self) -> LockHandle {
        LockHandle(Arc::clone(&self.shared))
    }

    /// A closure for `Session::on_lock`: it clears the caches and jobs.
    pub fn lock_hook(&self) -> impl FnOnce() + Send + 'static {
        let handle = self.lock_handle();
        move || handle.lock()
    }

    /// Asks for one picture. Never blocks on a decode: it returns the cached
    /// picture (and marks it as recently used), the earlier error, or queues
    /// a job and returns [`Status::Pending`].
    pub fn request(&self, kind: Kind, time: CaptureTime) -> Status {
        let mut st = self.shared.lock();
        if st.closed {
            return Status::Closed;
        }
        if let Some(img) = st.cache(kind).get(&time) {
            return Status::Done(Item::Ready(Arc::clone(img)));
        }
        if let Some(msg) = st.failed.get(&(kind, time)) {
            return Status::Done(Item::Failed(msg.clone()));
        }
        if st.wanted.insert((kind, time)) {
            st.queue.push((kind, time));
            drop(st);
            self.shared.wake.notify_one();
        }
        Status::Pending
    }

    /// Tells the loader where the user is. Queued jobs are served nearest
    /// first. Use this for steady scrolling; it keeps queued jobs.
    pub fn set_position(&self, time: CaptureTime) {
        self.shared.lock().position = time;
    }

    /// The user jumped to `time`: every queued job and every running job's
    /// result is dropped (stale), and the position moves.
    pub fn jump(&self, time: CaptureTime) {
        let mut st = self.shared.lock();
        st.position = time;
        st.drop_jobs();
    }

    /// Requests the pictures around `current`: [`PREFETCH_FULL`] full images
    /// and [`PREFETCH_THUMBS`] thumbnails on each side, and both kinds for
    /// `current` itself. Also sets the position. Never blocks.
    pub fn prefetch(&self, index: &Index, current: CaptureTime) {
        self.set_position(current);
        let times = index.times();
        let at = times.partition_point(|t| *t < current);
        let around = |n: usize| {
            let lo = at.saturating_sub(n);
            let hi = (at + n + 1).min(times.len());
            &times[lo..hi]
        };
        // The current position first (the worker order does the rest).
        self.request(Kind::Full, current);
        self.request(Kind::Thumb, current);
        for t in around(PREFETCH_FULL) {
            self.request(Kind::Full, *t);
        }
        for t in around(PREFETCH_THUMBS) {
            self.request(Kind::Thumb, *t);
        }
    }

    /// Takes the results delivered since the last call, oldest first.
    pub fn poll(&self) -> Vec<Loaded> {
        let rx = self.results.lock().unwrap_or_else(|p| p.into_inner());
        rx.try_iter().collect()
    }

    /// Empties the caches and the job queue (see [`LockHandle::lock`]).
    pub fn clear(&self) {
        self.lock_handle().lock();
    }

    /// Jobs queued or running in the current generation.
    pub fn pending(&self) -> usize {
        self.shared.lock().wanted.len()
    }

    /// Bytes in the (full image, thumbnail) caches.
    pub fn cached_bytes(&self) -> (usize, usize) {
        let st = self.shared.lock();
        (st.full.bytes(), st.thumbs.bytes())
    }

    /// Entries in the (full image, thumbnail) caches.
    pub fn cached_len(&self) -> (usize, usize) {
        let st = self.shared.lock();
        (st.full.len(), st.thumbs.len())
    }
}

impl Drop for ImageLoader {
    fn drop(&mut self) {
        self.lock_handle().lock();
        for w in self.workers.drain(..) {
            let _ = w.join();
        }
    }
}

fn worker(
    shared: &Shared,
    decoder: &dyn Decoder,
    tx: &Sender<Loaded>,
    repaint: &(dyn Fn() + Sync),
) {
    loop {
        let (key, generation) = {
            let mut st = shared.lock();
            loop {
                if st.closed {
                    return;
                }
                if let Some(key) = st.pop_nearest() {
                    break (key, st.generation);
                }
                st = shared.wake.wait(st).unwrap_or_else(|p| p.into_inner());
            }
        };
        let (kind, time) = key;
        // A panic in a decoder must not kill the worker or the viewer.
        let outcome =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| decoder.load(kind, time)))
                .unwrap_or_else(|_| Err("Decoding failed unexpectedly".to_string()));

        let item = {
            let mut st = shared.lock();
            if st.closed || st.generation != generation {
                continue; // stale: dropped, not cached, not delivered
            }
            st.wanted.remove(&key);
            match outcome {
                Ok(img) => {
                    let img = Arc::new(img);
                    st.cache(kind).insert(time, Arc::clone(&img));
                    Item::Ready(img)
                }
                Err(msg) => {
                    st.failed.insert(key, msg.clone());
                    Item::Failed(msg)
                }
            }
        };
        if tx.send(Loaded { time, kind, item }).is_err() {
            return; // the loader is gone
        }
        repaint();
    }
}
