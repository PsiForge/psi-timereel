//! TimeReel image pipeline: stitch monitors, scale, make thumbnails, encode and decode.
//!
//! Everything in this crate works on in-memory buffers. It never opens, creates
//! or writes a file, so no screenshot data can leak to disk through it
//! (architecture section 7). The pixel format is always 8-bit RGBA, row by row,
//! with no padding.
//!
//! The typical agent flow is [`stitch`] (which also scales to the target width),
//! [`thumbnail`], then [`encode`] for both. The viewer calls [`decode`].

use fast_image_resize::images::{Image, ImageRef};
use fast_image_resize::{FilterType, PixelType, ResizeAlg, ResizeOptions, Resizer};

/// Default output width in pixels (`width_px` in the config).
pub const DEFAULT_WIDTH_PX: u32 = 1280;
/// Default encoder quality of the full image (`quality` in the config).
pub const DEFAULT_QUALITY: u8 = 50;
/// Default thumbnail width in pixels (`thumbnail_width_px` in the config).
pub const DEFAULT_THUMBNAIL_WIDTH_PX: u32 = 240;
/// Default encoder quality of the thumbnail (ADR 0011).
pub const DEFAULT_THUMBNAIL_QUALITY: u8 = 40;

/// Largest number of pixels [`decode`] accepts (64 Mpixel, 256 MB of RGBA).
/// It protects the viewer from files that claim an absurd size.
pub const MAX_DECODE_PIXELS: u64 = 64 * 1024 * 1024;

/// Largest side of an image that both WebP (16383) and JPEG (65535) can store.
const MAX_ENCODE_SIDE: u32 = 16383;
/// Largest side of a stitched canvas, in screen pixels.
const MAX_CANVAS_SIDE: i64 = 1 << 20;

/// Errors of the image pipeline.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A width or height is zero, or too large.
    #[error("invalid image size {width}x{height}")]
    InvalidSize {
        /// Width in pixels.
        width: u32,
        /// Height in pixels.
        height: u32,
    },
    /// The pixel buffer does not hold exactly `width * height * 4` bytes.
    #[error("pixel buffer has {actual} bytes, expected {expected}")]
    BufferSize {
        /// Bytes needed for the stated size.
        expected: usize,
        /// Bytes that were passed.
        actual: usize,
    },
    /// `stitch` was called without any frame.
    #[error("no frames to stitch")]
    NoFrames,
    /// The target width is zero.
    #[error("target width must be at least 1")]
    InvalidTargetWidth,
    /// The quality is outside 1..=100.
    #[error("quality must be between 1 and 100, got {0}")]
    InvalidQuality(u8),
    /// The monitor layout is too large to handle.
    #[error("monitor layout is too large")]
    LayoutTooLarge,
    /// Scaling failed.
    #[error("scaling failed: {0}")]
    Resize(String),
    /// Encoding failed.
    #[error("encoding failed: {0}")]
    Encode(String),
    /// The data is not a valid WebP or JPEG image.
    #[error("decoding failed: {0}")]
    Decode(String),
}

/// Result type of this crate.
pub type Result<T> = std::result::Result<T, Error>;

/// An 8-bit RGBA image in memory.
///
/// The invariant `data.len() == width * height * 4` with non-zero sides is
/// checked when the value is created.
#[derive(Clone, PartialEq, Eq)]
pub struct RgbaImage {
    width: u32,
    height: u32,
    data: Vec<u8>,
}

impl std::fmt::Debug for RgbaImage {
    // Never print pixel data: it is screen content.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "RgbaImage({}x{})", self.width, self.height)
    }
}

fn rgba_len(width: u32, height: u32) -> Result<usize> {
    if width == 0 || height == 0 {
        return Err(Error::InvalidSize { width, height });
    }
    (width as usize)
        .checked_mul(height as usize)
        .and_then(|n| n.checked_mul(4))
        .ok_or(Error::InvalidSize { width, height })
}

impl RgbaImage {
    /// Wraps an RGBA buffer. Fails if a side is zero or the length is wrong.
    pub fn new(width: u32, height: u32, data: Vec<u8>) -> Result<Self> {
        let expected = rgba_len(width, height)?;
        if data.len() != expected {
            return Err(Error::BufferSize {
                expected,
                actual: data.len(),
            });
        }
        Ok(Self {
            width,
            height,
            data,
        })
    }

    /// Width in pixels.
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Height in pixels.
    pub fn height(&self) -> u32 {
        self.height
    }

    /// The RGBA bytes, row by row.
    pub fn data(&self) -> &[u8] {
        &self.data
    }

    /// Gives back the RGBA buffer.
    pub fn into_data(self) -> Vec<u8> {
        self.data
    }

    /// The RGBA value of one pixel, or `None` outside the image.
    pub fn pixel(&self, x: u32, y: u32) -> Option<[u8; 4]> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let i = (y as usize * self.width as usize + x as usize) * 4;
        let p = self.data.get(i..i + 4)?;
        Some([p[0], p[1], p[2], p[3]])
    }
}

/// One captured monitor: its RGBA pixels and where it sits on the desktop.
///
/// `x` and `y` are the top-left corner in desktop pixels and may be negative.
/// The alpha channel is ignored: screens are treated as opaque.
#[derive(Clone)]
pub struct Frame {
    /// Left edge on the desktop.
    pub x: i32,
    /// Top edge on the desktop.
    pub y: i32,
    /// The monitor pixels.
    pub image: RgbaImage,
}

impl Frame {
    /// A frame from an image and its position.
    pub fn new(x: i32, y: i32, image: RgbaImage) -> Self {
        Self { x, y, image }
    }
}

/// Image file format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// Lossy WebP (libwebp).
    Webp,
    /// Baseline JPEG.
    Jpeg,
}

/// Resizes RGBA pixels. Alpha is treated as a normal channel (no premultiply),
/// so frames with an undefined alpha byte do not turn black.
fn resize_rgba(
    resizer: &mut Resizer,
    src: &[u8],
    (sw, sh): (u32, u32),
    (dw, dh): (u32, u32),
) -> Result<Vec<u8>> {
    let src =
        ImageRef::new(sw, sh, src, PixelType::U8x4).map_err(|e| Error::Resize(e.to_string()))?;
    let mut dst = Image::new(dw, dh, PixelType::U8x4);
    let options = ResizeOptions::new()
        .resize_alg(ResizeAlg::Convolution(FilterType::Lanczos3))
        .use_alpha(false);
    resizer
        .resize(&src, &mut dst, &options)
        .map_err(|e| Error::Resize(e.to_string()))?;
    Ok(dst.into_vec())
}

/// Scales `image` to `width` pixels wide, keeping the aspect ratio.
///
/// It never upscales: an image that is already `width` or narrower is returned
/// unchanged (as a copy). The new height is rounded to the nearest pixel, so the
/// aspect ratio is kept within one pixel.
pub fn scale_to_width(image: &RgbaImage, width: u32) -> Result<RgbaImage> {
    if width == 0 {
        return Err(Error::InvalidTargetWidth);
    }
    if image.width <= width {
        return Ok(image.clone());
    }
    let height = scaled_len(image.height, width, image.width).max(1);
    let data = resize_rgba(
        &mut Resizer::new(),
        &image.data,
        (image.width, image.height),
        (width, height),
    )?;
    RgbaImage::new(width, height, data)
}

/// Makes a thumbnail `width` pixels wide (default [`DEFAULT_THUMBNAIL_WIDTH_PX`]).
/// Same rules as [`scale_to_width`]: aspect ratio kept, never upscaled.
pub fn thumbnail(image: &RgbaImage, width: u32) -> Result<RgbaImage> {
    scale_to_width(image, width)
}

/// `round(value * num / den)` without overflow.
fn scaled_len(value: u32, num: u32, den: u32) -> u32 {
    let v = (u64::from(value) * u64::from(num) + u64::from(den) / 2) / u64::from(den);
    // Callers pass num <= den or small values, so this always fits.
    u32::try_from(v).unwrap_or(u32::MAX)
}

/// Rounds `value * num / den` for a signed offset from the canvas origin.
fn scaled_edge(value: i64, num: u32, den: u32) -> i64 {
    let den = i64::from(den);
    (value * i64::from(num) + den / 2) / den
}

/// Stitches monitor frames into one image of at most `target_width` pixels.
///
/// - The canvas is the bounding box of all frames in desktop coordinates.
/// - Areas no monitor covers are opaque black.
/// - If the canvas is wider than `target_width`, every frame is scaled while it
///   is placed, so the full-size canvas is never allocated. The result is
///   exactly `target_width` wide and its height keeps the aspect ratio within
///   one pixel.
/// - A canvas that is not wider than `target_width` is not upscaled.
/// - If frames overlap, later frames are drawn over earlier ones.
/// - Alpha of the input is ignored; the output is fully opaque.
pub fn stitch(frames: &[Frame], target_width: u32) -> Result<RgbaImage> {
    if target_width == 0 {
        return Err(Error::InvalidTargetWidth);
    }
    if frames.is_empty() {
        return Err(Error::NoFrames);
    }

    let (mut min_x, mut min_y) = (i64::MAX, i64::MAX);
    let (mut max_x, mut max_y) = (i64::MIN, i64::MIN);
    for f in frames {
        min_x = min_x.min(i64::from(f.x));
        min_y = min_y.min(i64::from(f.y));
        max_x = max_x.max(i64::from(f.x) + i64::from(f.image.width));
        max_y = max_y.max(i64::from(f.y) + i64::from(f.image.height));
    }
    let (canvas_w, canvas_h) = (max_x - min_x, max_y - min_y);
    if canvas_w > MAX_CANVAS_SIDE || canvas_h > MAX_CANVAS_SIDE {
        return Err(Error::LayoutTooLarge);
    }
    let (canvas_w, canvas_h) = (canvas_w as u32, canvas_h as u32);

    let (out_w, out_h) = if canvas_w <= target_width {
        (canvas_w, canvas_h)
    } else {
        (
            target_width,
            scaled_len(canvas_h, target_width, canvas_w).max(1),
        )
    };
    let mut out = vec![0u8; rgba_len(out_w, out_h)?];
    for px in out.chunks_exact_mut(4) {
        px[3] = 255;
    }

    let mut resizer = Resizer::new();
    for f in frames {
        // Destination rectangle from rounded edges, so neighbouring monitors
        // share an edge and leave no gap.
        let left = scaled_edge(i64::from(f.x) - min_x, out_w, canvas_w);
        let top = scaled_edge(i64::from(f.y) - min_y, out_h, canvas_h);
        let right = scaled_edge(
            i64::from(f.x) - min_x + i64::from(f.image.width),
            out_w,
            canvas_w,
        );
        let bottom = scaled_edge(
            i64::from(f.y) - min_y + i64::from(f.image.height),
            out_h,
            canvas_h,
        );
        let dw = (right - left).max(1) as u32;
        let dh = (bottom - top).max(1) as u32;

        let scaled;
        let pixels: &[u8] = if (dw, dh) == (f.image.width, f.image.height) {
            &f.image.data
        } else {
            scaled = resize_rgba(
                &mut resizer,
                &f.image.data,
                (f.image.width, f.image.height),
                (dw, dh),
            )?;
            &scaled
        };

        // Blit with clipping to the canvas.
        let x0 = left.clamp(0, i64::from(out_w));
        let x1 = (left + i64::from(dw)).clamp(0, i64::from(out_w));
        let y0 = top.clamp(0, i64::from(out_h));
        let y1 = (top + i64::from(dh)).clamp(0, i64::from(out_h));
        if x0 >= x1 {
            continue;
        }
        let src_x0 = (x0 - left) as usize;
        let len = (x1 - x0) as usize;
        for y in y0..y1 {
            let src_row = (y - top) as usize * dw as usize;
            let src = &pixels[(src_row + src_x0) * 4..(src_row + src_x0 + len) * 4];
            let dst_start = (y as usize * out_w as usize + x0 as usize) * 4;
            let dst = &mut out[dst_start..dst_start + len * 4];
            dst.copy_from_slice(src);
            for px in dst.chunks_exact_mut(4) {
                px[3] = 255;
            }
        }
    }
    RgbaImage::new(out_w, out_h, out)
}

/// Encodes an image. `quality` is 1..=100 (defaults: [`DEFAULT_QUALITY`] for
/// images, [`DEFAULT_THUMBNAIL_QUALITY`] for thumbnails). Alpha is not stored.
pub fn encode(image: &RgbaImage, format: Format, quality: u8) -> Result<Vec<u8>> {
    if !(1..=100).contains(&quality) {
        return Err(Error::InvalidQuality(quality));
    }
    if image.width > MAX_ENCODE_SIDE || image.height > MAX_ENCODE_SIDE {
        return Err(Error::InvalidSize {
            width: image.width,
            height: image.height,
        });
    }
    match format {
        Format::Webp => {
            let encoder = webp::Encoder::from_rgba(&image.data, image.width, image.height);
            let out = encoder
                .encode_simple(false, f32::from(quality))
                .map_err(|e| Error::Encode(format!("libwebp: {e:?}")))?;
            Ok(out.to_vec())
        }
        Format::Jpeg => {
            let mut out = Vec::new();
            // Both sides are <= 16383, so the u16 casts are lossless.
            jpeg_encoder::Encoder::new(&mut out, quality)
                .encode(
                    &image.data,
                    image.width as u16,
                    image.height as u16,
                    jpeg_encoder::ColorType::Rgba,
                )
                .map_err(|e| Error::Encode(e.to_string()))?;
            Ok(out)
        }
    }
}

/// Decodes a WebP or JPEG image to RGBA. The format is detected from the
/// first bytes. Images above [`MAX_DECODE_PIXELS`] are refused.
pub fn decode(bytes: &[u8]) -> Result<RgbaImage> {
    if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        decode_webp(bytes)
    } else if bytes.starts_with(&[0xFF, 0xD8]) {
        decode_jpeg(bytes)
    } else {
        Err(Error::Decode("not a WebP or JPEG image".into()))
    }
}

fn check_decode_size(width: u32, height: u32) -> Result<()> {
    if width == 0 || height == 0 || u64::from(width) * u64::from(height) > MAX_DECODE_PIXELS {
        return Err(Error::InvalidSize { width, height });
    }
    Ok(())
}

fn decode_webp(bytes: &[u8]) -> Result<RgbaImage> {
    let features = webp::BitstreamFeatures::new(bytes)
        .ok_or_else(|| Error::Decode("invalid WebP header".into()))?;
    check_decode_size(features.width(), features.height())?;
    let img = webp::Decoder::new(bytes)
        .decode()
        .ok_or_else(|| Error::Decode("invalid or unsupported WebP data".into()))?;
    let (width, height) = (img.width(), img.height());
    let data = if img.is_alpha() {
        img.to_vec()
    } else {
        let mut rgba = Vec::with_capacity(img.len() / 3 * 4);
        for px in img.chunks_exact(3) {
            rgba.extend_from_slice(&[px[0], px[1], px[2], 255]);
        }
        rgba
    };
    RgbaImage::new(width, height, data)
}

fn decode_jpeg(bytes: &[u8]) -> Result<RgbaImage> {
    use zune_jpeg::zune_core::bytestream::ZCursor;
    use zune_jpeg::zune_core::colorspace::ColorSpace;
    use zune_jpeg::zune_core::options::DecoderOptions;

    let options = DecoderOptions::default().jpeg_set_out_colorspace(ColorSpace::RGBA);
    let mut decoder = zune_jpeg::JpegDecoder::new_with_options(ZCursor::new(bytes), options);
    decoder
        .decode_headers()
        .map_err(|e| Error::Decode(format!("{e:?}")))?;
    let info = decoder
        .info()
        .ok_or_else(|| Error::Decode("missing JPEG header".into()))?;
    let (width, height) = (u32::from(info.width), u32::from(info.height));
    check_decode_size(width, height)?;
    let data = decoder
        .decode()
        .map_err(|e| Error::Decode(format!("{e:?}")))?;
    RgbaImage::new(width, height, data)
}

#[cfg(test)]
mod tests;
