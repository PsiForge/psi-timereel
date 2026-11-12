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
//! Capture times are UTC milliseconds ([`CaptureTime`]); the day folders on
//! disk are UTC days too. "Days" in this crate are local days in a time zone
//! passed in by the caller (any [`chrono::TimeZone`]), so a local day may
//! span two UTC day folders and may be 23 or 25 hours long on a DST change.

mod gaps;
mod index;
mod jump;
mod localday;

#[cfg(test)]
mod tests;

pub use gaps::{find_gaps, Gap, GAP_FACTOR_PERCENT};
pub use index::{Index, Landing, REFRESH_INTERVAL};
pub use jump::QuickJump;
pub use localday::{local_date, local_day_bounds, local_day_start};
pub use timereel_store::CaptureTime;
