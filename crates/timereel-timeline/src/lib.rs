//! TimeReel `timeline` crate: the viewer logic that needs no GUI
//! (architecture sections 2.1 and 8, ADR 0002).
//!
//! This ticket covers the time index and what you can ask it:
//!
//! * [`Index`]: the sorted capture times, built from [`timereel_store::Store::list`]
//!   (file names only, no key needed) and refreshed incrementally with
//!   [`Index::refresh`]. The viewer calls it every [`REFRESH_INTERVAL`].
//! * Queries: [`Index::next`], [`Index::prev`], [`Index::range`] (one local
//!   day), [`Index::at_or_after`] and [`Index::nearest`].
//! * [`Index::quick_jump`] for the jump buttons ([`QuickJump`]).
//! * [`Index::gaps`] and [`find_gaps`]: breaks longer than 1.5 x the interval.
//!
//! The background loader ([`ImageLoader`]) decrypts and decodes pictures on
//! worker threads into byte-limited caches ([`ByteLru`]); a generation counter
//! drops stale jobs after [`ImageLoader::jump`]. [`ImageLoader::lock_hook`]
//! goes into the viewer's `Session::on_lock`.
//!
//! The timeline view (T20) builds on [`StripLayout`] (where each thumbnail is,
//! so only visible ones are drawn), [`plan_requests`] (which pictures to ask
//! the loader for), [`MetaStore`] (all metadata, decrypted in the background
//! and cleared on lock) and the label helpers [`format_clock`],
//! [`format_date_time`] and [`format_gap`].
//!
//! Search (T22): [`SearchFilter`] is a case-insensitive substring test on the
//! app name and the window title, applied to what the [`MetaStore`] has
//! loaded so far ([`MetaStore::matches`], [`MetaStore::matching`]);
//! [`Index::next_matching`] and [`Index::prev_matching`] move within the
//! matches. [`MetaStore::progress`] tells how far the loading is.
//!
//! Capture times are UTC milliseconds ([`CaptureTime`]); the day folders on
//! disk are UTC days too. "Days" in this crate are local days in a time zone
//! passed in by the caller (any [`chrono::TimeZone`]), so a local day may
//! span two UTC day folders and may be 23 or 25 hours long on a DST change.

mod format;
mod gaps;
mod index;
mod jump;
mod loader;
mod localday;
mod lru;
mod meta;
mod search;
mod strip;

#[cfg(test)]
mod loader_tests;
#[cfg(test)]
mod search_tests;
#[cfg(test)]
mod strip_tests;
#[cfg(test)]
mod tests;

pub use format::{format_clock, format_date_time, format_gap};
pub use gaps::{find_gaps, Gap, GAP_FACTOR_PERCENT};
pub use index::{Index, Landing, REFRESH_INTERVAL};
pub use jump::QuickJump;
pub use loader::{
    default_workers, Decoder, ImageLoader, Item, Kind, Loaded, LockHandle, Status, StoreDecoder,
    MAX_WORKERS, PREFETCH_FULL, PREFETCH_THUMBS,
};
pub use localday::{local_date, local_day_bounds, local_day_start};
pub use lru::{ByteLru, ByteSize, FULL_CACHE_BYTES, THUMB_CACHE_BYTES};
pub use meta::{MetaEntry, MetaSource, MetaStore, StoreMetaSource, REPAINT_EVERY};
pub use search::SearchFilter;
pub use strip::{plan_requests, RequestPlan, StripLayout};
pub use timereel_store::CaptureTime;
