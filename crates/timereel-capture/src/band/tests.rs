#![allow(clippy::unwrap_used)]

use super::*;

/// A desktop of `width` x `height` made of known colours, served in bands.
/// It counts the raw bytes it hands out, which stands in for an allocation
/// counter on the band buffer: every band is a fresh `Vec` that is built
/// here, passed to the consumer and dropped before the next call.
struct CountingSource {
    width: u32,
    height: u32,
    colour: fn(u32, u32) -> [u8; 3],
    calls: usize,
    allocated: usize,
    largest_band: usize,
    in_flight: bool,
    overlapped: bool,
}

impl CountingSource {
    fn new(width: u32, height: u32, colour: fn(u32, u32) -> [u8; 3]) -> Self {
        Self {
            width,
            height,
            colour,
            calls: 0,
            allocated: 0,
            largest_band: 0,
            in_flight: false,
            overlapped: false,
        }
    }
}

impl BandSource for CountingSource {
    fn read_band(
        &mut self,
        x: i32,
        y: i32,
        width: u32,
        rows: u32,
        consume: &mut dyn FnMut(&[u8]) -> Result<(), BandError>,
    ) -> Result<(), BandError> {
        assert!(
            x >= 0 && y >= 0 && x as u32 + width <= self.width && y as u32 + rows <= self.height
        );
        if self.in_flight {
            self.overlapped = true;
        }
        self.in_flight = true;
        let mut band = Vec::with_capacity((width * rows * 4) as usize);
        for r in 0..rows {
            for c in 0..width {
                let [red, green, blue] = (self.colour)(x as u32 + c, y as u32 + r);
                band.extend_from_slice(&[blue, green, red, 0]);
            }
        }
        self.calls += 1;
        self.allocated += band.len();
        self.largest_band = self.largest_band.max(band.len());
        let result = consume(&band);
        drop(band);
        self.in_flight = false;
        result
    }
}

fn halves(x: u32, _y: u32) -> [u8; 3] {
    if x < 1280 {
        [200, 10, 10]
    } else {
        [10, 10, 200]
    }
}

const FULL: Rect = Rect {
    x: 0,
    y: 0,
    width: 2560,
    height: 1080,
};

#[test]
fn band_capture_holds_at_most_one_band_of_raw_data() {
    let mut src = CountingSource::new(2560, 1080, halves);
    let frames = capture_scaled(&mut src, &[FULL], 1280).unwrap();

    let max_band = (MAX_BAND_ROWS * 2560 * 4) as usize;
    assert!(
        src.largest_band <= max_band,
        "band of {} bytes",
        src.largest_band
    );
    assert_eq!(src.calls, 5, "1080 rows in bands of at most 256 rows");
    assert!(
        !src.overlapped,
        "a band was still alive when the next was read"
    );
    // Everything was read once, in pieces, never as one frame.
    assert_eq!(src.allocated, 2560 * 1080 * 4);
    assert!(src.allocated > 3 * src.largest_band);

    assert_eq!(frames.len(), 1);
    let img = &frames[0].image;
    assert_eq!((img.width(), img.height()), (1280, 540));
    assert_eq!(img.pixel(10, 10).unwrap(), [200, 10, 10, 255]);
    assert_eq!(img.pixel(1270, 530).unwrap(), [10, 10, 200, 255]);
}

#[test]
fn two_monitors_are_placed_and_scaled_like_stitch() {
    let mut src = CountingSource::new(2560, 1080, halves);
    let monitors = [
        Rect {
            x: 0,
            y: 0,
            width: 1280,
            height: 1080,
        },
        Rect {
            x: 1280,
            y: 0,
            width: 1280,
            height: 1080,
        },
    ];
    let frames = capture_scaled(&mut src, &monitors, 1280).unwrap();
    assert_eq!((frames[0].x, frames[0].y), (0, 0));
    assert_eq!((frames[1].x, frames[1].y), (640, 0));
    assert_eq!(frames[0].image.width(), 640);
    assert_eq!(frames[1].image.width(), 640);

    let stitched = timereel_image::stitch(&frames, 1280).unwrap();
    assert_eq!((stitched.width(), stitched.height()), (1280, 540));
    assert_eq!(stitched.pixel(5, 5).unwrap(), [200, 10, 10, 255]);
    assert_eq!(stitched.pixel(1275, 5).unwrap(), [10, 10, 200, 255]);
}

#[test]
fn small_layouts_are_not_upscaled_and_averaging_is_exact() {
    let mut src = CountingSource::new(100, 40, |_, _| [7, 8, 9]);
    let monitors = [Rect {
        x: 0,
        y: 0,
        width: 100,
        height: 40,
    }];
    let frames = capture_scaled(&mut src, &monitors, 1280).unwrap();
    let img = &frames[0].image;
    assert_eq!((img.width(), img.height()), (100, 40));
    assert_eq!(img.pixel(99, 39).unwrap(), [7, 8, 9, 255]);

    // A 2x2 average of a checkerboard is the middle value.
    let mut src = CountingSource::new(
        4,
        4,
        |x, y| {
            if (x + y) % 2 == 0 {
                [0; 3]
            } else {
                [255; 3]
            }
        },
    );
    let monitors = [Rect {
        x: 0,
        y: 0,
        width: 4,
        height: 4,
    }];
    let frames = capture_scaled(&mut src, &monitors, 2).unwrap();
    assert_eq!(frames[0].image.pixel(0, 0).unwrap(), [128, 128, 128, 255]);
}

#[test]
fn bad_layouts_and_data_are_errors() {
    let mut src = CountingSource::new(10, 10, |_, _| [0; 3]);
    assert!(matches!(
        capture_scaled(&mut src, &[], 100),
        Err(BandError::Layout)
    ));
    let empty = [Rect {
        x: 0,
        y: 0,
        width: 0,
        height: 5,
    }];
    assert!(matches!(
        capture_scaled(&mut src, &empty, 100),
        Err(BandError::Layout)
    ));
    let ok = [Rect {
        x: 0,
        y: 0,
        width: 10,
        height: 10,
    }];
    assert!(matches!(
        capture_scaled(&mut src, &ok, 0),
        Err(BandError::Layout)
    ));

    let mut scaler = MonitorScaler::new(4, 4, 4, 4).unwrap();
    assert!(matches!(scaler.push_band(&[0; 7]), Err(BandError::BadData)));
    assert!(matches!(scaler.finish(), Err(BandError::BadData)));
}

#[test]
fn a_failing_source_stops_the_capture() {
    struct Failing;
    impl BandSource for Failing {
        fn read_band(
            &mut self,
            _: i32,
            _: i32,
            _: u32,
            _: u32,
            _: &mut dyn FnMut(&[u8]) -> Result<(), BandError>,
        ) -> Result<(), BandError> {
            Err(BandError::Source("test"))
        }
    }
    let ok = [Rect {
        x: 0,
        y: 0,
        width: 10,
        height: 10,
    }];
    assert!(matches!(
        capture_scaled(&mut Failing, &ok, 100),
        Err(BandError::Source("test"))
    ));
}
