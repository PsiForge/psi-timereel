//! Band-wise capture and scaling of monitors (ADR 0007, architecture 9).
//!
//! A monitor is read in horizontal bands of at most [`MAX_BAND_ROWS`] rows.
//! Each band is converted from BGRx and added to the scaled output as soon as
//! it arrives, then dropped, so the raw data held at any time is one band and
//! never a full multi-monitor frame.

use timereel_image::{Frame, RgbaImage};

/// The most rows read in one band.
pub const MAX_BAND_ROWS: u32 = 256;

/// The largest canvas side accepted, in pixels. Bigger layouts are almost
/// certainly a bug in the monitor data.
const MAX_CANVAS_SIDE: i64 = 32_768;

/// A monitor rectangle in desktop coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    /// Left edge.
    pub x: i32,
    /// Top edge.
    pub y: i32,
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
}

/// Why a band capture failed.
#[derive(Debug, thiserror::Error)]
pub enum BandError {
    /// The monitor layout is empty, has an empty monitor or is too large.
    #[error("unusable monitor layout")]
    Layout,
    /// The pixel data did not have the expected size.
    #[error("unexpected pixel data size")]
    BadData,
    /// The source failed; the string is a fixed kind, never content.
    #[error("band source failed: {0}")]
    Source(&'static str),
}

/// Reads one band of raw BGRx pixels. The band is handed to `consume` and
/// must not outlive the call, which is what keeps memory at one band.
pub trait BandSource {
    /// Reads the `width` x `rows` pixels at (`x`, `y`) in desktop
    /// coordinates, 4 bytes per pixel (blue, green, red, unused).
    fn read_band(
        &mut self,
        x: i32,
        y: i32,
        width: u32,
        rows: u32,
        consume: &mut dyn FnMut(&[u8]) -> Result<(), BandError>,
    ) -> Result<(), BandError>;
}

/// Box-filter scaler for one monitor, fed band by band. Source row `sy` goes
/// to destination row `sy * dst_h / src_h` and source column `sx` to
/// `sx * dst_w / src_w`; each destination pixel is the average of the source
/// pixels that map to it. It only scales down (or keeps the size).
struct MonitorScaler {
    src_w: u32,
    src_h: u32,
    dst_w: u32,
    dst_h: u32,
    /// Destination column of every source column.
    col_map: Vec<u32>,
    /// Number of source columns per destination column.
    col_counts: Vec<u32>,
    /// Channel sums (r, g, b) of the destination row being built.
    sums: Vec<u32>,
    rows_in_current: u32,
    current_dst_row: u32,
    next_src_row: u32,
    out: Vec<u8>,
}

impl MonitorScaler {
    fn new(src_w: u32, src_h: u32, dst_w: u32, dst_h: u32) -> Result<Self, BandError> {
        if dst_w == 0 || dst_h == 0 || dst_w > src_w || dst_h > src_h {
            return Err(BandError::Layout);
        }
        let col_map: Vec<u32> = (0..src_w)
            .map(|sx| (u64::from(sx) * u64::from(dst_w) / u64::from(src_w)) as u32)
            .collect();
        let mut col_counts = vec![0u32; dst_w as usize];
        for &c in &col_map {
            col_counts[c as usize] += 1;
        }
        let out_len = (dst_w as usize)
            .checked_mul(dst_h as usize)
            .and_then(|n| n.checked_mul(4))
            .ok_or(BandError::Layout)?;
        Ok(Self {
            src_w,
            src_h,
            dst_w,
            dst_h,
            col_map,
            col_counts,
            sums: vec![0; dst_w as usize * 3],
            rows_in_current: 0,
            current_dst_row: 0,
            next_src_row: 0,
            out: vec![255; out_len],
        })
    }

    fn dst_row_of(&self, src_row: u32) -> u32 {
        (u64::from(src_row) * u64::from(self.dst_h) / u64::from(self.src_h)) as u32
    }

    fn flush_row(&mut self) {
        if self.rows_in_current == 0 {
            return;
        }
        let row_start = self.current_dst_row as usize * self.dst_w as usize * 4;
        for dc in 0..self.dst_w as usize {
            let n = self.col_counts[dc] * self.rows_in_current;
            let px = &mut self.out[row_start + dc * 4..row_start + dc * 4 + 4];
            for (out, sum) in px.iter_mut().zip(&self.sums[dc * 3..dc * 3 + 3]) {
                // The average of 8-bit values fits in 8 bits; add n/2 to round.
                *out = ((sum + n / 2) / n) as u8;
            }
            px[3] = 255;
        }
        self.sums.fill(0);
        self.rows_in_current = 0;
    }

    /// Adds a band of whole BGRx rows.
    fn push_band(&mut self, band: &[u8]) -> Result<(), BandError> {
        let stride = self.src_w as usize * 4;
        if band.is_empty() || band.len() % stride != 0 {
            return Err(BandError::BadData);
        }
        let rows = band.len() / stride;
        if self.next_src_row as usize + rows > self.src_h as usize {
            return Err(BandError::BadData);
        }
        for row in band.chunks_exact(stride) {
            let dr = self.dst_row_of(self.next_src_row);
            if dr != self.current_dst_row {
                self.flush_row();
                self.current_dst_row = dr;
            }
            for (px, &dc) in row.chunks_exact(4).zip(&self.col_map) {
                let s = dc as usize * 3;
                self.sums[s] += u32::from(px[2]);
                self.sums[s + 1] += u32::from(px[1]);
                self.sums[s + 2] += u32::from(px[0]);
            }
            self.rows_in_current += 1;
            self.next_src_row += 1;
        }
        Ok(())
    }

    fn finish(mut self) -> Result<RgbaImage, BandError> {
        if self.next_src_row != self.src_h {
            return Err(BandError::BadData);
        }
        self.flush_row();
        RgbaImage::new(self.dst_w, self.dst_h, self.out).map_err(|_| BandError::Layout)
    }
}

/// `round(value * num / den)` for a non-negative `value`.
fn scaled(value: i64, num: u32, den: u32) -> i64 {
    let den = i64::from(den);
    (value * i64::from(num) + den / 2) / den
}

/// Captures all `monitors` through `source` and scales them so the combined
/// layout is at most `target_width` wide (never upscaled).
///
/// The frames come back already scaled, positioned relative to the top-left
/// of the layout with the same rounding as `timereel_image::stitch`, so
/// stitching them to the same width places them unchanged.
pub fn capture_scaled(
    source: &mut dyn BandSource,
    monitors: &[Rect],
    target_width: u32,
) -> Result<Vec<Frame>, BandError> {
    if target_width == 0
        || monitors.is_empty()
        || monitors.iter().any(|m| m.width == 0 || m.height == 0)
    {
        return Err(BandError::Layout);
    }
    let min_x = monitors.iter().map(|m| i64::from(m.x)).min().unwrap_or(0);
    let min_y = monitors.iter().map(|m| i64::from(m.y)).min().unwrap_or(0);
    let max_x = monitors
        .iter()
        .map(|m| i64::from(m.x) + i64::from(m.width))
        .max()
        .unwrap_or(0);
    let max_y = monitors
        .iter()
        .map(|m| i64::from(m.y) + i64::from(m.height))
        .max()
        .unwrap_or(0);
    let (canvas_w, canvas_h) = (max_x - min_x, max_y - min_y);
    if canvas_w > MAX_CANVAS_SIDE || canvas_h > MAX_CANVAS_SIDE {
        return Err(BandError::Layout);
    }
    let (canvas_w, canvas_h) = (canvas_w as u32, canvas_h as u32);
    let (out_w, out_h) = if canvas_w <= target_width {
        (canvas_w, canvas_h)
    } else {
        (
            target_width,
            scaled(i64::from(canvas_h), target_width, canvas_w).max(1) as u32,
        )
    };

    let mut frames = Vec::with_capacity(monitors.len());
    for m in monitors {
        let left = scaled(i64::from(m.x) - min_x, out_w, canvas_w);
        let top = scaled(i64::from(m.y) - min_y, out_h, canvas_h);
        let right = scaled(i64::from(m.x) - min_x + i64::from(m.width), out_w, canvas_w);
        let bottom = scaled(
            i64::from(m.y) - min_y + i64::from(m.height),
            out_h,
            canvas_h,
        );
        // Rounding can make a destination one pixel larger than the source
        // when nothing is scaled; this scaler never enlarges.
        let dst_w = ((right - left).max(1) as u32).min(m.width);
        let dst_h = ((bottom - top).max(1) as u32).min(m.height);

        let mut scaler = MonitorScaler::new(m.width, m.height, dst_w, dst_h)?;
        let mut row = 0;
        while row < m.height {
            let rows = (m.height - row).min(MAX_BAND_ROWS);
            source.read_band(m.x, m.y + row as i32, m.width, rows, &mut |band| {
                scaler.push_band(band)
            })?;
            row += rows;
        }
        frames.push(Frame::new(left as i32, top as i32, scaler.finish()?));
    }
    Ok(frames)
}

#[cfg(test)]
mod tests;
