//! TimeReel benchmark helpers (ADR 0013): statistics, thresholds and the
//! deterministic synthetic desktop images. The runner is `src/main.rs`.

use std::time::Duration;
use timereel_image::{RgbaImage, DEFAULT_WIDTH_PX};

pub mod data;

/// Warm-up runs before measuring.
pub const WARMUP_RUNS: usize = 20;
/// Measured runs per series.
pub const MEASURED_RUNS: usize = 200;

/// Median, 95th percentile and maximum of a series, in milliseconds.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Stats {
    /// Median (ms).
    pub median: f64,
    /// 95th percentile, nearest rank (ms).
    pub p95: f64,
    /// Maximum (ms).
    pub max: f64,
}

/// Computes [`Stats`]. Returns `None` for an empty sample.
pub fn stats(samples: &[Duration]) -> Option<Stats> {
    if samples.is_empty() {
        return None;
    }
    let mut ms: Vec<f64> = samples.iter().map(|d| d.as_secs_f64() * 1000.0).collect();
    ms.sort_by(f64::total_cmp);
    let n = ms.len();
    let median = if n % 2 == 1 {
        ms[n / 2]
    } else {
        (ms[n / 2 - 1] + ms[n / 2]) / 2.0
    };
    let rank = (n * 95).div_ceil(100).max(1);
    Some(Stats {
        median,
        p95: ms[rank - 1],
        max: ms[n - 1],
    })
}

/// Limits for one series; `None` means not gated.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Limit {
    /// Median limit (ms).
    pub median: Option<f64>,
    /// p95 limit (ms).
    pub p95: Option<f64>,
}

/// The ADR 0013 limits.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Thresholds {
    /// Full image decrypt + decode.
    pub full: Limit,
    /// Thumbnail decrypt + decode.
    pub thumb: Limit,
    /// Cold jump through the loader.
    pub cold_jump: Limit,
}

impl Default for Thresholds {
    fn default() -> Self {
        Self {
            full: Limit {
                median: Some(40.0),
                p95: Some(100.0),
            },
            thumb: Limit {
                median: Some(5.0),
                p95: Some(10.0),
            },
            cold_jump: Limit {
                median: None,
                p95: Some(120.0),
            },
        }
    }
}

impl Thresholds {
    /// Defaults, with overrides from the environment (in ms):
    /// `TIMEREEL_BENCH_FULL_MEDIAN_MS`, `..._FULL_P95_MS`, `..._THUMB_MEDIAN_MS`,
    /// `..._THUMB_P95_MS`, `..._COLD_JUMP_P95_MS`. Invalid values are an error.
    pub fn from_env() -> Result<Self, String> {
        Self::from_lookup(|k| std::env::var(k).ok())
    }

    /// Like [`Thresholds::from_env`] with a custom lookup (for tests).
    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Result<Self, String> {
        let mut t = Self::default();
        let pick = |name: &str, slot: &mut Option<f64>| -> Result<(), String> {
            if let Some(v) = get(name) {
                let x: f64 = v
                    .trim()
                    .parse()
                    .map_err(|_| format!("{name} must be a number of milliseconds"))?;
                if !x.is_finite() || x < 0.0 {
                    return Err(format!("{name} must be a non-negative number"));
                }
                *slot = Some(x);
            }
            Ok(())
        };
        pick("TIMEREEL_BENCH_FULL_MEDIAN_MS", &mut t.full.median)?;
        pick("TIMEREEL_BENCH_FULL_P95_MS", &mut t.full.p95)?;
        pick("TIMEREEL_BENCH_THUMB_MEDIAN_MS", &mut t.thumb.median)?;
        pick("TIMEREEL_BENCH_THUMB_P95_MS", &mut t.thumb.p95)?;
        pick("TIMEREEL_BENCH_COLD_JUMP_P95_MS", &mut t.cold_jump.p95)?;
        Ok(t)
    }
}

/// Checks one series against its limit. Returns one message per exceeded
/// limit; empty means the series passes.
pub fn check(name: &str, s: &Stats, limit: &Limit) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(m) = limit.median {
        if s.median > m {
            out.push(format!(
                "{name}: median {:.2} ms exceeds {m:.2} ms",
                s.median
            ));
        }
    }
    if let Some(p) = limit.p95 {
        if s.p95 > p {
            out.push(format!("{name}: p95 {:.2} ms exceeds {p:.2} ms", s.p95));
        }
    }
    out
}

/// A deterministic desktop-like picture: window frames, a title bar, text-like
/// lines, a gradient and mild noise. The same seed gives the same pixels.
pub fn synthetic_desktop(width: u32, height: u32, seed: u64) -> Result<RgbaImage, String> {
    let (w, h) = (width as usize, height as usize);
    let mut data = vec![255u8; w * h * 4];
    let mut rng = seed ^ 0x9E37_79B9_7F4A_7C15;
    let mut next = move || {
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        rng
    };
    for y in 0..h {
        for x in 0..w {
            let i = (y * w + x) * 4;
            let (mut r, mut g, mut b) = (
                (40 + x * 60 / w) as u8,
                (60 + y * 80 / h) as u8,
                (110 + (x + y) * 50 / (w + h)) as u8,
            );
            // A window with a title bar covering most of the screen.
            if x > w / 16 && x < w * 15 / 16 && y > h / 12 && y < h * 11 / 12 {
                let bar = y < h / 12 + h / 30;
                (r, g, b) = if bar { (70, 70, 80) } else { (245, 245, 245) };
                // Text-like lines: dark dashes on every 24th row band.
                if !bar && (y / 6) % 4 == 0 && ((x / 7) + (y / 24)) % 5 != 0 {
                    (r, g, b) = (40, 40, 40);
                }
            }
            let n = (next() & 3) as u8;
            data[i] = r.saturating_add(n);
            data[i + 1] = g.saturating_add(n);
            data[i + 2] = b.saturating_add(n);
        }
    }
    RgbaImage::new(width, height, data).map_err(|e| e.to_string())
}

/// The default stored width, re-exported to make the "defaults" explicit.
pub const STORED_WIDTH_PX: u32 = DEFAULT_WIDTH_PX;

#[cfg(test)]
mod tests;
