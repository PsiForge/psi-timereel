//! Converting PipeWire video buffers to RGBA images.

use timereel_image::RgbaImage;

/// Largest width or height of a stream frame that is accepted. This guards the
/// allocation against a broken or hostile size from the stream.
pub const MAX_SIDE: u32 = 16_384;

/// The pixel layouts of a PipeWire SHM frame that the Wayland source accepts.
/// Names follow SPA: the letters are the byte order in memory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixelFormat {
    /// Bytes `B, G, R, x` (the padding byte is ignored).
    Bgrx,
    /// Bytes `R, G, B, x` (the padding byte is ignored).
    Rgbx,
    /// Bytes `B, G, R, A`. Screens are opaque, so alpha is ignored.
    Bgra,
    /// Bytes `R, G, B, A`. Screens are opaque, so alpha is ignored.
    Rgba,
}

/// Why a frame could not be converted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum FrameError {
    /// The width or height is zero or larger than [`MAX_SIDE`].
    #[error("invalid frame size")]
    Size,
    /// The stride is smaller than one row of pixels (or not positive).
    #[error("invalid frame stride")]
    Stride,
    /// The buffer is shorter than `stride * height` needs.
    #[error("frame buffer too short")]
    Short,
}

/// Converts one frame to RGBA.
///
/// `stride` is the distance in bytes between the starts of two rows; it may be
/// larger than `width * 4` (padding at the end of each row, which is skipped).
/// The last row only needs `width * 4` bytes. The output is fully opaque.
///
/// # Errors
/// [`FrameError`] if the size, stride or buffer length does not fit.
pub fn convert_to_rgba(
    format: PixelFormat,
    width: u32,
    height: u32,
    stride: i64,
    data: &[u8],
) -> Result<RgbaImage, FrameError> {
    if width == 0 || height == 0 || width > MAX_SIDE || height > MAX_SIDE {
        return Err(FrameError::Size);
    }
    let row_bytes = width as usize * 4;
    let stride = usize::try_from(stride).map_err(|_| FrameError::Stride)?;
    if stride < row_bytes {
        return Err(FrameError::Stride);
    }
    let needed = stride
        .checked_mul(height as usize - 1)
        .and_then(|n| n.checked_add(row_bytes))
        .ok_or(FrameError::Short)?;
    if data.len() < needed {
        return Err(FrameError::Short);
    }
    let mut out = Vec::with_capacity(row_bytes * height as usize);
    for row in 0..height as usize {
        let line = &data[row * stride..row * stride + row_bytes];
        for px in line.chunks_exact(4) {
            let rgb = match format {
                PixelFormat::Bgrx | PixelFormat::Bgra => [px[2], px[1], px[0]],
                PixelFormat::Rgbx | PixelFormat::Rgba => [px[0], px[1], px[2]],
            };
            out.extend_from_slice(&[rgb[0], rgb[1], rgb[2], 255]);
        }
    }
    // The size was checked above, so the image cannot be rejected.
    RgbaImage::new(width, height, out).map_err(|_| FrameError::Size)
}
