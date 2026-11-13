//! The geometry of the thumbnail strip and what it asks the loader for.
//!
//! The strip is one long row of thumbnails with a marker slot after every
//! break in the recording. [`StripLayout`] knows where each thumbnail is, so
//! drawing finds the visible ones by binary search and never touches the
//! rest (virtualisation). [`plan_requests`] says which thumbnails and full
//! images the viewer may ask [`crate::ImageLoader`] for.

use std::ops::Range;
use std::time::Duration;

use timereel_store::CaptureTime;

use crate::gaps::{find_gaps, Gap};
use crate::loader::{PREFETCH_FULL, PREFETCH_THUMBS};

/// Positions of the thumbnails and gap markers along the strip (x only, in
/// points, from the left end of the strip).
#[derive(Debug, Clone)]
pub struct StripLayout {
    lefts: Vec<f32>,
    /// `(index of the capture before the break, the break)`, by index.
    gaps: Vec<(usize, Gap)>,
    item_width: f32,
    gap_width: f32,
    spacing: f32,
    total: f32,
}

impl StripLayout {
    /// The layout of `times` (sorted, oldest first). A break is two
    /// neighbours more than 1.5 x `interval` apart.
    pub fn new(
        times: &[CaptureTime],
        interval: Duration,
        item_width: f32,
        gap_width: f32,
        spacing: f32,
    ) -> Self {
        let gaps: Vec<(usize, Gap)> = find_gaps(times, interval)
            .into_iter()
            .filter_map(|g| {
                let i = times.binary_search(&g.before).ok()?;
                Some((i, g))
            })
            .collect();
        let mut lefts = Vec::with_capacity(times.len());
        let mut x = 0.0_f32;
        let mut next_gap = gaps.iter().map(|(i, _)| *i).peekable();
        for i in 0..times.len() {
            lefts.push(x);
            x += item_width + spacing;
            if next_gap.peek() == Some(&i) {
                next_gap.next();
                x += gap_width + spacing;
            }
        }
        Self {
            lefts,
            gaps,
            item_width,
            gap_width,
            spacing,
            total: x,
        }
    }

    /// How many thumbnails.
    pub fn len(&self) -> usize {
        self.lefts.len()
    }

    /// Whether there are no thumbnails.
    pub fn is_empty(&self) -> bool {
        self.lefts.is_empty()
    }

    /// The whole length of the strip.
    pub fn total_width(&self) -> f32 {
        self.total
    }

    /// The width of one thumbnail slot.
    pub fn item_width(&self) -> f32 {
        self.item_width
    }

    /// The width of one gap marker.
    pub fn gap_width(&self) -> f32 {
        self.gap_width
    }

    /// The left edge of thumbnail `i`.
    pub fn left(&self, i: usize) -> f32 {
        self.lefts.get(i).copied().unwrap_or(self.total)
    }

    /// The middle of thumbnail `i`.
    pub fn center(&self, i: usize) -> f32 {
        self.left(i) + self.item_width / 2.0
    }

    /// The left edge of the marker that follows thumbnail `i`.
    pub fn gap_left(&self, i: usize) -> f32 {
        self.left(i) + self.item_width + self.spacing
    }

    /// The thumbnails that show in a window `width` wide whose left edge is
    /// at `scroll_x` (partly visible ones count).
    pub fn visible(&self, scroll_x: f32, width: f32) -> Range<usize> {
        let start = self
            .lefts
            .partition_point(|&l| l + self.item_width <= scroll_x);
        let end = self.lefts.partition_point(|&l| l < scroll_x + width);
        start..end.max(start)
    }

    /// The breaks whose marker may show when the thumbnails `visible` show.
    pub fn gaps_near(&self, visible: Range<usize>) -> &[(usize, Gap)] {
        let from = visible.start.saturating_sub(1);
        let lo = self.gaps.partition_point(|(i, _)| *i < from);
        let hi = self.gaps.partition_point(|(i, _)| *i < visible.end);
        &self.gaps[lo..hi.max(lo)]
    }

    /// The thumbnail under the strip position `x`, if any (a click on the
    /// space between thumbnails or on a marker finds nothing).
    pub fn hit(&self, x: f32) -> Option<usize> {
        let i = self.lefts.partition_point(|&l| l <= x).checked_sub(1)?;
        (x < self.left(i) + self.item_width).then_some(i)
    }
}

/// Which pictures the viewer may request, as ranges of capture indexes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestPlan {
    /// Thumbnails: the visible ones plus [`PREFETCH_THUMBS`] on each side.
    pub thumbs: Range<usize>,
    /// Full images: the selected capture plus [`PREFETCH_FULL`] on each side.
    pub full: Range<usize>,
}

/// What to request when the strip shows the captures `visible` (indexes),
/// capture `selected` is selected and there are `len` captures. The selected
/// capture's thumbnail is always included (it is the placeholder while the
/// full image loads). Empty when `len` is 0.
pub fn plan_requests(visible: Range<usize>, selected: usize, len: usize) -> RequestPlan {
    if len == 0 {
        return RequestPlan {
            thumbs: 0..0,
            full: 0..0,
        };
    }
    let selected = selected.min(len - 1);
    let (lo, hi) = if visible.is_empty() {
        (selected, selected + 1)
    } else {
        (visible.start.min(selected), visible.end.max(selected + 1))
    };
    RequestPlan {
        thumbs: lo.saturating_sub(PREFETCH_THUMBS)..(hi + PREFETCH_THUMBS).min(len),
        full: selected.saturating_sub(PREFETCH_FULL)..(selected + PREFETCH_FULL + 1).min(len),
    }
}
