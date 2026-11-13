//! The unlocked state of the viewer (architecture section 2.4).
//!
//! A [`Session`] holds the opened private key and everything derived from
//! decrypted data. Caches and metadata register an [`Session::on_lock`] hook
//! that clears them, and GPU textures are handed to [`Session::keep_texture`].
//! [`Session::lock`] (and dropping the session) runs the hooks, frees the
//! textures and drops the key, which zeroizes it.

use timereel_crypto::UnlockedKey;

/// A clean-up step that runs once when the session locks.
pub type LockHook = Box<dyn FnOnce() + Send>;

/// All decrypted state of one unlock. `K` is the key type; the real program
/// uses [`UnlockedKey`], tests use a double that records its wiping.
pub struct Session<K = UnlockedKey> {
    key: Option<K>,
    hooks: Vec<LockHook>,
    textures: Vec<egui::TextureHandle>,
}

impl<K> Session<K> {
    /// A session that owns `key`.
    pub fn new(key: K) -> Self {
        Self {
            key: Some(key),
            hooks: Vec::new(),
            textures: Vec::new(),
        }
    }

    /// The key, or `None` once the session is locked.
    pub fn key(&self) -> Option<&K> {
        self.key.as_ref()
    }

    /// True until [`Session::lock`] runs.
    pub fn is_unlocked(&self) -> bool {
        self.key.is_some()
    }

    /// Registers a hook that runs when the session locks. If the session is
    /// already locked, the hook runs at once.
    pub fn on_lock(&mut self, hook: impl FnOnce() + Send + 'static) {
        if self.is_unlocked() {
            self.hooks.push(Box::new(hook));
        } else {
            hook();
        }
    }

    /// Keeps a GPU texture alive until the session locks.
    pub fn keep_texture(&mut self, texture: egui::TextureHandle) {
        if self.is_unlocked() {
            self.textures.push(texture);
        }
    }

    /// Runs every hook (in the order they were registered), frees the
    /// textures and drops the key. Calling it again does nothing.
    pub fn lock(&mut self) {
        for hook in self.hooks.drain(..) {
            hook();
        }
        self.textures.clear();
        self.key = None;
    }
}

impl<K> Drop for Session<K> {
    fn drop(&mut self) {
        self.lock();
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    use zeroize::Zeroize;

    use super::*;

    /// Stands in for the private key and wipes a shared buffer on drop, so
    /// the test can look at the memory after the key is gone.
    struct SecretDouble(Arc<Mutex<[u8; 32]>>);

    impl Drop for SecretDouble {
        fn drop(&mut self) {
            self.0.lock().unwrap().zeroize();
        }
    }

    #[test]
    fn lock_runs_all_hooks_and_wipes_the_key() {
        let buffer = Arc::new(Mutex::new([0x5a; 32]));
        let mut session = Session::new(SecretDouble(buffer.clone()));
        let ran = Arc::new(AtomicUsize::new(0));
        for _ in 0..3 {
            let ran = ran.clone();
            session.on_lock(move || {
                ran.fetch_add(1, Ordering::SeqCst);
            });
        }
        assert!(session.is_unlocked());
        assert_eq!(*buffer.lock().unwrap(), [0x5a; 32]);

        session.lock();
        assert_eq!(ran.load(Ordering::SeqCst), 3);
        assert!(session.key().is_none());
        assert!(!session.is_unlocked());
        assert_eq!(*buffer.lock().unwrap(), [0; 32], "secret buffer wiped");

        // Locking twice runs nothing again; late hooks run at once.
        session.lock();
        assert_eq!(ran.load(Ordering::SeqCst), 3);
        let late = ran.clone();
        session.on_lock(move || {
            late.fetch_add(10, Ordering::SeqCst);
        });
        assert_eq!(ran.load(Ordering::SeqCst), 13);
    }

    #[test]
    fn dropping_the_session_locks_it() {
        let buffer = Arc::new(Mutex::new([7; 32]));
        let ran = Arc::new(AtomicUsize::new(0));
        let mut session = Session::new(SecretDouble(buffer.clone()));
        let r = ran.clone();
        session.on_lock(move || {
            r.fetch_add(1, Ordering::SeqCst);
        });
        drop(session);
        assert_eq!(ran.load(Ordering::SeqCst), 1);
        assert_eq!(*buffer.lock().unwrap(), [0; 32]);
    }

    #[test]
    fn lock_frees_the_textures() {
        let ctx = egui::Context::default();
        let before = ctx.tex_manager().read().num_allocated();
        let mut session = Session::new(());
        let image = egui::ColorImage::filled([4, 4], egui::Color32::RED);
        session.keep_texture(ctx.load_texture("capture", image, Default::default()));
        assert_eq!(ctx.tex_manager().read().num_allocated(), before + 1);
        session.lock();
        assert_eq!(ctx.tex_manager().read().num_allocated(), before);
    }
}
