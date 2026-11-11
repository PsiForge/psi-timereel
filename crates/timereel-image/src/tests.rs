#![allow(clippy::unwrap_used)]

use super::*;

const RED: [u8; 3] = [255, 0, 0];
const GREEN: [u8; 3] = [0, 255, 0];
const BLUE: [u8; 3] = [0, 0, 255];
const BLACK: [u8; 4] = [0, 0, 0, 255];

fn solid(w: u32, h: u32, c: [u8; 3]) -> RgbaImage {
    let data = (0..w * h).flat_map(|_| [c[0], c[1], c[2], 255]).collect();
    RgbaImage::new(w, h, data).unwrap()
}

fn frame(x: i32, y: i32, w: u32, h: u32, c: [u8; 3]) -> Frame {
    Frame::new(x, y, solid(w, h, c))
}

fn px(img: &RgbaImage, x: u32, y: u32) -> [u8; 4] {
    img.pixel(x, y).unwrap()
}

fn opaque(c: [u8; 3]) -> [u8; 4] {
    [c[0], c[1], c[2], 255]
}

/// A smooth gradient with some detail, so that encoders have real work to do.
fn gradient(w: u32, h: u32) -> RgbaImage {
    let mut data = Vec::new();
    for y in 0..h {
        for x in 0..w {
            data.extend_from_slice(&[
                (x * 255 / w) as u8,
                (y * 255 / h) as u8,
                ((x ^ y) & 0xff) as u8,
                255,
            ]);
        }
    }
    RgbaImage::new(w, h, data).unwrap()
}

// ---- stitching: layout and pixel placement (no scaling: target is wide) ----

#[test]
fn side_by_side() {
    let out = stitch(
        &[frame(0, 0, 100, 50, RED), frame(100, 0, 100, 50, BLUE)],
        4000,
    )
    .unwrap();
    assert_eq!((out.width(), out.height()), (200, 50));
    assert_eq!(px(&out, 0, 0), opaque(RED));
    assert_eq!(px(&out, 99, 49), opaque(RED));
    assert_eq!(px(&out, 100, 0), opaque(BLUE));
    assert_eq!(px(&out, 199, 49), opaque(BLUE));
}

#[test]
fn stacked() {
    let out = stitch(
        &[frame(0, 0, 100, 50, RED), frame(0, 50, 100, 70, GREEN)],
        4000,
    )
    .unwrap();
    assert_eq!((out.width(), out.height()), (100, 120));
    assert_eq!(px(&out, 50, 49), opaque(RED));
    assert_eq!(px(&out, 50, 50), opaque(GREEN));
    assert_eq!(px(&out, 99, 119), opaque(GREEN));
}

#[test]
fn offset_leaves_black_gaps() {
    // B sits 20 px lower than A, so there is black above B and below A.
    let out = stitch(
        &[frame(0, 0, 100, 50, RED), frame(100, 20, 100, 50, BLUE)],
        4000,
    )
    .unwrap();
    assert_eq!((out.width(), out.height()), (200, 70));
    assert_eq!(px(&out, 10, 10), opaque(RED));
    assert_eq!(px(&out, 150, 10), BLACK); // above B
    assert_eq!(px(&out, 150, 20), opaque(BLUE));
    assert_eq!(px(&out, 50, 60), BLACK); // below A
    assert_eq!(px(&out, 199, 69), opaque(BLUE));
}

#[test]
fn different_heights() {
    let out = stitch(
        &[frame(0, 0, 100, 100, RED), frame(100, 0, 100, 50, BLUE)],
        4000,
    )
    .unwrap();
    assert_eq!((out.width(), out.height()), (200, 100));
    assert_eq!(px(&out, 150, 49), opaque(BLUE));
    assert_eq!(px(&out, 150, 50), BLACK);
    assert_eq!(px(&out, 50, 99), opaque(RED));
}

#[test]
fn negative_positions_and_order_independence() {
    // The monitor on the left has a negative x, as in a real multi-monitor setup.
    let out = stitch(
        &[frame(0, 0, 100, 50, BLUE), frame(-100, -10, 100, 50, RED)],
        4000,
    )
    .unwrap();
    assert_eq!((out.width(), out.height()), (200, 60));
    assert_eq!(px(&out, 0, 0), opaque(RED));
    assert_eq!(px(&out, 99, 49), opaque(RED));
    assert_eq!(px(&out, 100, 10), opaque(BLUE));
    assert_eq!(px(&out, 100, 0), BLACK);
    assert_eq!(px(&out, 0, 59), BLACK);
}

#[test]
fn single_frame_is_copied_unchanged() {
    let src = gradient(64, 48);
    let out = stitch(&[Frame::new(5, 7, src.clone())], 1280).unwrap();
    assert_eq!(out, src);
}

#[test]
fn alpha_of_input_is_ignored() {
    let img = RgbaImage::new(2, 1, vec![9, 8, 7, 0, 1, 2, 3, 0]).unwrap();
    let out = stitch(&[Frame::new(0, 0, img)], 100).unwrap();
    assert_eq!(px(&out, 0, 0), [9, 8, 7, 255]);
    assert_eq!(px(&out, 1, 0), [1, 2, 3, 255]);
    // Scaling does not premultiply either: a transparent-alpha frame keeps its colour.
    let img = RgbaImage::new(4, 4, [200, 100, 50, 0].repeat(16)).unwrap();
    let out = stitch(&[Frame::new(0, 0, img)], 2).unwrap();
    assert_eq!(px(&out, 0, 0), [200, 100, 50, 255]);
}

// ---- stitching with scaling ----

#[test]
fn scaled_side_by_side_keeps_placement() {
    let out = stitch(
        &[
            frame(0, 0, 1920, 1080, RED),
            frame(1920, 0, 1920, 1080, BLUE),
        ],
        1280,
    )
    .unwrap();
    assert_eq!((out.width(), out.height()), (1280, 360));
    assert_eq!(px(&out, 10, 10), opaque(RED));
    assert_eq!(px(&out, 639, 180), opaque(RED));
    assert_eq!(px(&out, 640, 180), opaque(BLUE));
    assert_eq!(px(&out, 1270, 350), opaque(BLUE));
}

#[test]
fn scaled_offset_layout_has_black_and_no_gap() {
    let out = stitch(
        &[
            frame(0, 0, 1920, 1080, RED),
            frame(1920, 540, 1920, 1080, BLUE),
        ],
        1280,
    )
    .unwrap();
    assert_eq!((out.width(), out.height()), (1280, 540));
    assert_eq!(px(&out, 1000, 10), BLACK); // above B
    assert_eq!(px(&out, 10, 500), BLACK); // below A
    assert_eq!(px(&out, 639, 100), opaque(RED));
    assert_eq!(px(&out, 640, 400), opaque(BLUE));
    // No seam of black between the two monitors where they overlap in y.
    for y in 180..360 {
        assert_ne!(px(&out, 639, y), BLACK);
        assert_ne!(px(&out, 640, y), BLACK);
    }
}

#[test]
fn output_width_is_1280_and_aspect_kept_within_one_pixel() {
    for (w, h) in [
        (1920, 1080),
        (2560, 1440),
        (3840, 2160),
        (1366, 768),
        (1281, 1000),
        (5760, 1080),
        (3440, 1440),
    ] {
        let out = stitch(&[frame(0, 0, w, h, RED)], DEFAULT_WIDTH_PX).unwrap();
        assert_eq!(out.width(), 1280, "{w}x{h}");
        let exact = f64::from(h) * 1280.0 / f64::from(w);
        assert!(
            (f64::from(out.height()) - exact).abs() <= 1.0,
            "{w}x{h} -> {}",
            out.height()
        );
        assert_eq!(px(&out, 0, 0), opaque(RED));
    }
}

#[test]
fn narrower_source_is_not_upscaled() {
    let out = stitch(&[frame(0, 0, 1000, 600, RED)], 1280).unwrap();
    assert_eq!((out.width(), out.height()), (1000, 600));
    let out = stitch(&[frame(0, 0, 1280, 720, RED)], 1280).unwrap();
    assert_eq!((out.width(), out.height()), (1280, 720));
    let small = solid(100, 60, RED);
    assert_eq!(scale_to_width(&small, 1280).unwrap(), small);
    assert_eq!(thumbnail(&small, 240).unwrap(), small);
}

#[test]
fn thumbnail_has_requested_width_and_aspect() {
    let img = gradient(1280, 720);
    let t = thumbnail(&img, DEFAULT_THUMBNAIL_WIDTH_PX).unwrap();
    assert_eq!((t.width(), t.height()), (240, 135));
    let t = thumbnail(&gradient(1280, 360), 240).unwrap();
    assert_eq!((t.width(), t.height()), (240, 68)); // 67.5 rounds up
}

#[test]
fn very_wide_layout_never_gets_zero_height() {
    let out = stitch(&[frame(0, 0, 10_000, 1, RED)], 100).unwrap();
    assert_eq!((out.width(), out.height()), (100, 1));
}

#[test]
fn stitch_errors() {
    assert!(matches!(stitch(&[], 1280), Err(Error::NoFrames)));
    assert!(matches!(
        stitch(&[frame(0, 0, 2, 2, RED)], 0),
        Err(Error::InvalidTargetWidth)
    ));
    let far = [frame(0, 0, 2, 2, RED), frame(i32::MAX, 0, 2, 2, RED)];
    assert!(matches!(stitch(&far, 1280), Err(Error::LayoutTooLarge)));
}

#[test]
fn rgba_image_validates_its_buffer() {
    assert!(matches!(
        RgbaImage::new(0, 5, vec![]),
        Err(Error::InvalidSize { .. })
    ));
    assert!(matches!(
        RgbaImage::new(2, 2, vec![0; 15]),
        Err(Error::BufferSize {
            expected: 16,
            actual: 15
        })
    ));
    assert!(RgbaImage::new(2, 2, vec![0; 16]).is_ok());
    assert_eq!(format!("{:?}", solid(2, 2, RED)), "RgbaImage(2x2)");
}

// ---- encode and decode ----

#[test]
fn encode_decode_dimensions_webp_and_jpeg() {
    for (w, h) in [(1280, 720), (240, 135), (1, 1), (17, 33), (1280, 360)] {
        let img = gradient(w, h);
        for format in [Format::Webp, Format::Jpeg] {
            let bytes = encode(&img, format, 50).unwrap();
            let back = decode(&bytes).unwrap();
            assert_eq!((back.width(), back.height()), (w, h), "{format:?} {w}x{h}");
            assert_eq!(back.data().len(), (w * h * 4) as usize);
            assert!(back.data().chunks_exact(4).all(|p| p[3] == 255));
        }
    }
}

#[test]
fn encoded_data_has_the_right_format_and_colours_survive() {
    let img = solid(64, 64, [200, 40, 40]);
    let webp = encode(&img, Format::Webp, 50).unwrap();
    assert_eq!(&webp[..4], b"RIFF");
    let jpeg = encode(&img, Format::Jpeg, 50).unwrap();
    assert_eq!(&jpeg[..2], &[0xFF, 0xD8]);
    for bytes in [webp, jpeg] {
        let p = px(&decode(&bytes).unwrap(), 32, 32);
        assert!((i32::from(p[0]) - 200).abs() < 12, "{p:?}");
        assert!((i32::from(p[1]) - 40).abs() < 12, "{p:?}");
        assert!((i32::from(p[2]) - 40).abs() < 12, "{p:?}");
    }
}

#[test]
fn encode_rejects_bad_quality_and_size() {
    let img = solid(8, 8, RED);
    assert!(matches!(
        encode(&img, Format::Webp, 0),
        Err(Error::InvalidQuality(0))
    ));
    assert!(matches!(
        encode(&img, Format::Jpeg, 101),
        Err(Error::InvalidQuality(101))
    ));
    let wide = RgbaImage::new(16384, 1, vec![0; 16384 * 4]).unwrap();
    assert!(matches!(
        encode(&wide, Format::Webp, 50),
        Err(Error::InvalidSize { .. })
    ));
}

#[test]
fn decode_rejects_garbage_and_truncated_data() {
    assert!(matches!(decode(b""), Err(Error::Decode(_))));
    assert!(matches!(
        decode(b"not an image at all"),
        Err(Error::Decode(_))
    ));
    assert!(matches!(
        decode(b"RIFF\0\0\0\0WEBPxxxx"),
        Err(Error::Decode(_))
    ));
    let img = gradient(64, 64);
    for format in [Format::Webp, Format::Jpeg] {
        let bytes = encode(&img, format, 50).unwrap();
        // Truncated files must give an error or a valid image, never a panic.
        if let Ok(back) = decode(&bytes[..bytes.len() / 2]) {
            assert_eq!((back.width(), back.height()), (64, 64));
        }
        assert!(decode(&bytes[..10]).is_err());
    }
}

#[test]
fn decode_refuses_oversized_images() {
    // A lossless 16383x16383 WebP would be 1 GB of RGBA: build the header only.
    let mut bytes = encode(&solid(8, 8, RED), Format::Webp, 50).unwrap();
    // VP8 key frame header: width/height are 14-bit fields at offset 26..30.
    assert_eq!(&bytes[12..16], b"VP8 ");
    bytes[26..28].copy_from_slice(&16383u16.to_le_bytes());
    bytes[28..30].copy_from_slice(&16383u16.to_le_bytes());
    assert!(matches!(decode(&bytes), Err(Error::InvalidSize { .. })));
}
