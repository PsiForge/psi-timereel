//! Builds the temporary store through the real pipeline at default settings.

use crate::{synthetic_desktop, STORED_WIDTH_PX};
use age::x25519::Identity;
use std::path::Path;
use timereel_image::{
    encode, stitch, thumbnail, Format, Frame, DEFAULT_QUALITY, DEFAULT_THUMBNAIL_QUALITY,
    DEFAULT_THUMBNAIL_WIDTH_PX,
};
use timereel_store::{CaptureMeta, CaptureTime, Store};

/// Source sizes of the synthetic desktops.
pub const SOURCES: [(u32, u32); 2] = [(2560, 1440), (3840, 1080)];

/// Writes one capture per source size into a store under `dir`, using the
/// real stitch, thumbnail and encode steps at defaults (1280 px, WebP quality
/// 50, 240 px thumbnails). Returns the store and the capture times.
pub fn build_store(dir: &Path, public_key: &str) -> Result<(Store, Vec<CaptureTime>), String> {
    let store = Store::new(dir);
    let mut times = Vec::new();
    for (i, (w, h)) in SOURCES.iter().enumerate() {
        let src = synthetic_desktop(*w, *h, i as u64 + 1)?;
        let image = stitch(&[Frame::new(0, 0, src)], STORED_WIDTH_PX).map_err(|e| e.to_string())?;
        let thumb = thumbnail(&image, DEFAULT_THUMBNAIL_WIDTH_PX).map_err(|e| e.to_string())?;
        let image_bytes =
            encode(&image, Format::Webp, DEFAULT_QUALITY).map_err(|e| e.to_string())?;
        let thumb_bytes =
            encode(&thumb, Format::Webp, DEFAULT_THUMBNAIL_QUALITY).map_err(|e| e.to_string())?;
        let meta = CaptureMeta {
            taken_at_ms: 1_700_000_000_000 + (i as i64) * 60_000,
            tz_offset_min: 0,
            app_name: "bench".into(),
            window_title: "bench".into(),
            width: image.width(),
            height: image.height(),
            format: "webp".into(),
        };
        let t = store
            .write(public_key, &meta, &thumb_bytes, &image_bytes)
            .map_err(|e| e.to_string())?;
        times.push(t);
    }
    Ok((store, times))
}

/// Keeps the identity type in the signature of callers explicit.
pub type BenchIdentity = Identity;
