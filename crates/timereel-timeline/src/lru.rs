//! A least-recently-used cache that is limited by bytes, not by entries.

use std::collections::{BTreeMap, HashMap};
use std::hash::Hash;
use std::sync::Arc;

use timereel_image::RgbaImage;

/// Limit of the cache of decoded full images (architecture section 8).
pub const FULL_CACHE_BYTES: usize = 256 * 1024 * 1024;
/// Limit of the cache of decoded thumbnails (architecture section 8).
pub const THUMB_CACHE_BYTES: usize = 64 * 1024 * 1024;

/// Something with a size in bytes that the cache can count.
pub trait ByteSize {
    /// Bytes this value occupies in memory (the payload that matters).
    fn byte_size(&self) -> usize;
}

impl ByteSize for RgbaImage {
    fn byte_size(&self) -> usize {
        self.data().len()
    }
}

impl<T: ByteSize + ?Sized> ByteSize for Arc<T> {
    fn byte_size(&self) -> usize {
        (**self).byte_size()
    }
}

/// A cache whose total size never goes above a byte limit. When an insert
/// would exceed it, the least recently used entries are removed first.
/// Both [`ByteLru::get`] and [`ByteLru::insert`] count as a use.
#[derive(Debug)]
pub struct ByteLru<K, V> {
    limit: usize,
    bytes: usize,
    tick: u64,
    entries: HashMap<K, (V, u64)>,
    /// Use order: the smallest tick is the least recently used entry.
    order: BTreeMap<u64, K>,
}

impl<K: Hash + Eq + Clone, V: ByteSize> ByteLru<K, V> {
    /// An empty cache that holds at most `limit` bytes.
    pub fn new(limit: usize) -> Self {
        Self {
            limit,
            bytes: 0,
            tick: 0,
            entries: HashMap::new(),
            order: BTreeMap::new(),
        }
    }

    /// The byte limit.
    pub fn limit(&self) -> usize {
        self.limit
    }

    /// Bytes held now. Always at or below [`ByteLru::limit`].
    pub fn bytes(&self) -> usize {
        self.bytes
    }

    /// Number of entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// True if the cache holds nothing.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// True if `key` is cached. It does not count as a use.
    pub fn contains(&self, key: &K) -> bool {
        self.entries.contains_key(key)
    }

    fn next_tick(&mut self) -> u64 {
        self.tick += 1;
        self.tick
    }

    /// The value for `key`; marks it as the most recently used.
    pub fn get(&mut self, key: &K) -> Option<&V> {
        let tick = self.next_tick();
        let (_, old) = self.entries.get_mut(key).map(|e| {
            let old = e.1;
            e.1 = tick;
            ((), old)
        })?;
        self.order.remove(&old);
        self.order.insert(tick, key.clone());
        self.entries.get(key).map(|(v, _)| v)
    }

    /// Stores `value` as the most recently used entry and evicts the least
    /// recently used ones until the limit holds again. A value that is bigger
    /// than the whole limit is not stored (and returns `false`).
    pub fn insert(&mut self, key: K, value: V) -> bool {
        self.remove(&key);
        let size = value.byte_size();
        if size > self.limit {
            return false;
        }
        while self.bytes + size > self.limit {
            let Some((_, oldest)) = self.order.pop_first() else {
                break;
            };
            if let Some((v, _)) = self.entries.remove(&oldest) {
                self.bytes -= v.byte_size();
            }
        }
        let tick = self.next_tick();
        self.order.insert(tick, key.clone());
        self.entries.insert(key, (value, tick));
        self.bytes += size;
        true
    }

    /// Removes one entry.
    pub fn remove(&mut self, key: &K) -> Option<V> {
        let (value, tick) = self.entries.remove(key)?;
        self.order.remove(&tick);
        self.bytes -= value.byte_size();
        Some(value)
    }

    /// Drops every entry, so the memory is freed.
    pub fn clear(&mut self) {
        self.entries.clear();
        self.order.clear();
        self.bytes = 0;
    }
}
