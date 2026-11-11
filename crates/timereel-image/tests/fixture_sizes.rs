//! Size check on committed fixture screenshots (`tests/fixtures/*.png`).
//!
//! The test reads the fixtures from disk; the library itself never does.

#![allow(clippy::unwrap_used)]

use std::path::PathBuf;
use timereel_image::{
    decode, encode, stitch, thumbnail, Format, Frame, RgbaImage, DEFAULT_QUALITY,
    DEFAULT_THUMBNAIL_QUALITY, DEFAULT_THUMBNAIL_WIDTH_PX, DEFAULT_WIDTH_PX,
};

const MAX_MEAN_IMAGE_BYTES: f64 = 45.0 * 1000.0;
const MAX_MEAN_THUMB_BYTES: f64 = 6.0 * 1000.0;

fn fixtures() -> Vec<(String, RgbaImage)> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures");
    let mut paths: Vec<_> = std::fs::read_dir(&dir)
        .expect("fixtures dir")
        .map(|e| e.expect("dir entry").path())
        .filter(|p| p.extension().is_some_and(|e| e == "png"))
        .collect();
    paths.sort();
    assert!(
        paths.len() >= 3,
        "need at least 3 fixtures, found {}",
        paths.len()
    );
    paths
        .into_iter()
        .map(|p| {
            let mut decoder = png::Decoder::new(std::io::BufReader::new(
                std::fs::File::open(&p).expect("open fixture"),
            ));
            decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::ALPHA);
            let mut reader = decoder.read_info().expect("png header");
            let mut buf = vec![0; reader.output_buffer_size().expect("png size")];
            let info = reader.next_frame(&mut buf).expect("png data");
            buf.truncate(info.buffer_size());
            let rgba = match info.color_type {
                png::ColorType::Rgba => buf,
                png::ColorType::Rgb => buf
                    .chunks_exact(3)
                    .flat_map(|p| [p[0], p[1], p[2], 255])
                    .collect(),
                other => panic!("unsupported fixture colour type {other:?}"),
            };
            let name = p.file_name().unwrap().to_string_lossy().into_owned();
            (name, RgbaImage::new(info.width, info.height, rgba).unwrap())
        })
        .collect()
}

#[test]
fn mean_sizes_at_defaults_stay_within_budget() {
    for format in [Format::Webp, Format::Jpeg] {
        let (mut image_total, mut thumb_total, mut n) = (0usize, 0usize, 0usize);
        println!("format {format:?}");
        for (name, shot) in fixtures() {
            let stitched = stitch(&[Frame::new(0, 0, shot.clone())], DEFAULT_WIDTH_PX).unwrap();
            assert_eq!(stitched.width(), shot.width().min(DEFAULT_WIDTH_PX));
            let thumb = thumbnail(&stitched, DEFAULT_THUMBNAIL_WIDTH_PX).unwrap();
            assert_eq!(thumb.width(), DEFAULT_THUMBNAIL_WIDTH_PX);

            let image_bytes = encode(&stitched, format, DEFAULT_QUALITY).unwrap();
            let thumb_bytes = encode(&thumb, format, DEFAULT_THUMBNAIL_QUALITY).unwrap();
            println!(
                "  {name}: {}x{} -> image {}x{} {} B, thumbnail {}x{} {} B",
                shot.width(),
                shot.height(),
                stitched.width(),
                stitched.height(),
                image_bytes.len(),
                thumb.width(),
                thumb.height(),
                thumb_bytes.len()
            );
            let back = decode(&image_bytes).unwrap();
            assert_eq!(
                (back.width(), back.height()),
                (stitched.width(), stitched.height())
            );
            image_total += image_bytes.len();
            thumb_total += thumb_bytes.len();
            n += 1;
        }
        let (mean_image, mean_thumb) =
            (image_total as f64 / n as f64, thumb_total as f64 / n as f64);
        println!(
            "  mean image {mean_image:.0} B, mean thumbnail {mean_thumb:.0} B over {n} fixtures"
        );
        // The 45 KB / 6 KB budget is the WebP default (ADR 0011). JPEG is only reported.
        if format == Format::Webp {
            assert!(
                mean_image <= MAX_MEAN_IMAGE_BYTES,
                "mean image {mean_image:.0} B"
            );
            assert!(
                mean_thumb <= MAX_MEAN_THUMB_BYTES,
                "mean thumbnail {mean_thumb:.0} B"
            );
        }
    }
}
