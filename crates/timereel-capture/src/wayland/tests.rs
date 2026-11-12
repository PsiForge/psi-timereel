#![allow(clippy::unwrap_used)]

use std::sync::Arc;

use super::fake::{FakePortal, FakeStream};
use super::frame::{convert_to_rgba, FrameError, PixelFormat};
use super::*;

fn screen(portal: &Arc<FakePortal>, dir: &tempfile::TempDir) -> WaylandScreen<Arc<FakePortal>> {
    WaylandScreen::new(
        Arc::clone(portal),
        TokenStore::new(dir.path().join("state").join("portal-restore-token")),
    )
}

fn token_of(dir: &tempfile::TempDir) -> Option<String> {
    TokenStore::new(dir.path().join("state").join("portal-restore-token")).load()
}

#[test]
fn the_stored_token_is_passed_in_and_replaced_after_a_start() {
    let dir = tempfile::tempdir().unwrap();
    let portal = Arc::new(FakePortal::new(5));
    portal.accept_token("stored");
    let tokens = TokenStore::new(dir.path().join("state").join("portal-restore-token"));
    tokens.save("stored").unwrap();
    let source = WaylandScreen::new(Arc::clone(&portal), tokens);

    assert_eq!(source.capture().unwrap().len(), 1);
    assert_eq!(portal.received_tokens(), vec![Some("stored".to_string())]);
    assert_eq!(portal.prompts(), 0, "a valid token needs no dialog");
    assert_eq!(token_of(&dir).as_deref(), Some("token-1"));

    // The next capture passes the replaced token, and replaces it again.
    source.capture().unwrap();
    assert_eq!(
        portal.received_tokens(),
        vec![Some("stored".to_string()), Some("token-1".to_string())]
    );
    assert_eq!(token_of(&dir).as_deref(), Some("token-2"));
    assert_eq!(portal.prompts(), 0);
    assert_eq!(portal.closed(), 2, "every session is closed");
}

#[test]
fn a_first_capture_without_a_token_prompts_once_and_saves_the_new_token() {
    let dir = tempfile::tempdir().unwrap();
    let portal = Arc::new(FakePortal::new(4));
    let source = screen(&portal, &dir);
    source.capture().unwrap();
    assert_eq!(portal.received_tokens(), vec![None]);
    assert_eq!(portal.prompts(), 1);
    assert_eq!(token_of(&dir).as_deref(), Some("token-1"));
    source.capture().unwrap();
    assert_eq!(portal.prompts(), 1, "silent from then on");
}

#[test]
fn a_rejected_token_leads_to_one_prompt_and_needs_permission() {
    let dir = tempfile::tempdir().unwrap();
    let portal = Arc::new(FakePortal::new(5));
    portal.accept_token("the-real-one");
    portal.set_grant(false);
    let tokens = TokenStore::new(dir.path().join("state").join("portal-restore-token"));
    tokens.save("stale").unwrap();
    let source = WaylandScreen::new(Arc::clone(&portal), tokens);

    let error = source.capture().err();
    assert_eq!(error, Some(CaptureError::NoPermission));
    assert_eq!(portal.prompts(), 1);
    assert!(source.is_denied());
    // The agent maps NoPermission to needs_permission (see the agent tests).
}

#[test]
fn after_a_denial_there_are_no_prompts_until_retry_permission() {
    let dir = tempfile::tempdir().unwrap();
    let portal = Arc::new(FakePortal::new(5));
    portal.set_grant(false);
    let source = screen(&portal, &dir);

    assert_eq!(source.capture().err(), Some(CaptureError::NoPermission));
    for _ in 0..3 {
        assert_eq!(source.capture().err(), Some(CaptureError::NoPermission));
    }
    assert_eq!(portal.prompts(), 1);
    assert_eq!(portal.starts(), 1, "the portal is not even called");

    // The user grants this time and asks again from the tray.
    portal.set_grant(true);
    assert_eq!(source.capture().err(), Some(CaptureError::NoPermission));
    source.retry_permission();
    assert!(!source.is_denied());
    assert_eq!(source.capture().unwrap().len(), 1);
    assert_eq!(portal.prompts(), 2);
    assert_eq!(portal.starts(), 2);
}

#[test]
fn retry_permission_without_a_denial_changes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let portal = Arc::new(FakePortal::new(5));
    let source = screen(&portal, &dir);
    source.retry_permission();
    assert_eq!(portal.starts(), 0);
    assert_eq!(source.capture().unwrap().len(), 1);
}

#[test]
fn a_portal_below_version_4_gives_a_clear_error() {
    let dir = tempfile::tempdir().unwrap();
    for version in [1, 3] {
        let portal = Arc::new(FakePortal::new(version));
        let source = screen(&portal, &dir);
        let error = source.capture().err().unwrap();
        assert_eq!(error, CaptureError::PortalTooOld);
        assert_eq!(
            error.to_string(),
            "screen capture portal is older than version 4"
        );
        assert_eq!(portal.starts(), 0, "no session is opened");
        assert!(!source.is_denied());
    }
}

#[test]
fn bgrx_with_padded_stride() {
    // 2x2 pixels, stride 12 (4 bytes of padding per row, filled with 0xEE).
    let data = [
        1, 2, 3, 0, 4, 5, 6, 0, 0xEE, 0xEE, 0xEE, 0xEE, // row 0: B G R x ...
        7, 8, 9, 0, 10, 11, 12, 0, 0xEE, 0xEE, 0xEE, 0xEE,
    ];
    let img = convert_to_rgba(PixelFormat::Bgrx, 2, 2, 12, &data).unwrap();
    assert_eq!(img.pixel(0, 0), Some([3, 2, 1, 255]));
    assert_eq!(img.pixel(1, 0), Some([6, 5, 4, 255]));
    assert_eq!(img.pixel(0, 1), Some([9, 8, 7, 255]));
    assert_eq!(img.pixel(1, 1), Some([12, 11, 10, 255]));
}

#[test]
fn rgbx_with_padded_stride_and_a_short_last_row() {
    // The last row has no padding: the buffer is stride * (h - 1) + w * 4.
    let data = [
        1, 2, 3, 99, 4, 5, 6, 99, 0xEE, 0xEE, 0xEE, 0xEE, //
        7, 8, 9, 99, 10, 11, 12, 99,
    ];
    let img = convert_to_rgba(PixelFormat::Rgbx, 2, 2, 12, &data).unwrap();
    assert_eq!(img.pixel(0, 0), Some([1, 2, 3, 255]));
    assert_eq!(img.pixel(1, 0), Some([4, 5, 6, 255]));
    assert_eq!(img.pixel(0, 1), Some([7, 8, 9, 255]));
    assert_eq!(img.pixel(1, 1), Some([10, 11, 12, 255]));
}

#[test]
fn alpha_formats_become_opaque() {
    let bgra = convert_to_rgba(PixelFormat::Bgra, 1, 1, 4, &[1, 2, 3, 7]).unwrap();
    assert_eq!(bgra.pixel(0, 0), Some([3, 2, 1, 255]));
    let rgba = convert_to_rgba(PixelFormat::Rgba, 1, 1, 4, &[1, 2, 3, 7]).unwrap();
    assert_eq!(rgba.pixel(0, 0), Some([1, 2, 3, 255]));
}

#[test]
fn bad_frames_are_refused() {
    let ok = [0u8; 64];
    let f = PixelFormat::Bgrx;
    assert_eq!(
        convert_to_rgba(f, 0, 2, 8, &ok).err(),
        Some(FrameError::Size)
    );
    assert_eq!(
        convert_to_rgba(f, 100_000, 1, 8, &ok).err(),
        Some(FrameError::Size)
    );
    assert_eq!(
        convert_to_rgba(f, 4, 2, 8, &ok).err(),
        Some(FrameError::Stride)
    );
    assert_eq!(
        convert_to_rgba(f, 4, 2, -16, &ok).err(),
        Some(FrameError::Stride)
    );
    assert_eq!(
        convert_to_rgba(f, 4, 4, 16, &ok[..40]).err(),
        Some(FrameError::Short)
    );
    assert_eq!(
        convert_to_rgba(f, 4, 2, i64::MAX, &ok).err(),
        Some(FrameError::Short)
    );
}

#[test]
fn a_two_stream_layout_is_stitched_correctly() {
    let dir = tempfile::tempdir().unwrap();
    // The right monitor is listed first and the left one is taller, with
    // padded rows, so the order and the positions both matter.
    let mut left = FakeStream::solid(2, (0, 0), 4, 4, [255, 0, 0]);
    left.stride = 24;
    left.data = [
        0, 0, 255, 0, 0, 0, 255, 0, 0, 0, 255, 0, 0, 0, 255, 0, 9, 9, 9, 9, 9, 9, 9, 9,
    ]
    .repeat(4);
    let right = FakeStream::solid(1, (4, 0), 2, 2, [0, 0, 255]);
    let portal = Arc::new(FakePortal::with_streams(5, vec![right, left]));
    let frames = screen(&portal, &dir).capture().unwrap();
    assert_eq!(frames.len(), 2);

    let stitched = timereel_image::stitch(&frames, 1280).unwrap();
    assert_eq!((stitched.width(), stitched.height()), (6, 4));
    assert_eq!(stitched.pixel(0, 0), Some([255, 0, 0, 255]));
    assert_eq!(stitched.pixel(3, 3), Some([255, 0, 0, 255]));
    assert_eq!(stitched.pixel(4, 0), Some([0, 0, 255, 255]));
    assert_eq!(stitched.pixel(5, 1), Some([0, 0, 255, 255]));
    // Below the shorter monitor nothing is covered: opaque black.
    assert_eq!(stitched.pixel(5, 3), Some([0, 0, 0, 255]));
}

#[test]
fn streams_without_positions_are_placed_left_to_right() {
    let dir = tempfile::tempdir().unwrap();
    let mut a = FakeStream::solid(1, (0, 0), 3, 2, [1, 1, 1]);
    let mut b = FakeStream::solid(2, (0, 0), 2, 2, [2, 2, 2]);
    a.info.position = None;
    b.info.position = None;
    let portal = Arc::new(FakePortal::with_streams(5, vec![a, b]));
    let frames = screen(&portal, &dir).capture().unwrap();
    assert_eq!((frames[0].x, frames[1].x), (0, 3));
}

#[test]
fn a_hidpi_stream_is_scaled_to_its_logical_size() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = FakeStream::solid(1, (0, 0), 8, 4, [5, 5, 5]);
    s.info.size = Some((4, 2));
    let portal = Arc::new(FakePortal::with_streams(5, vec![s]));
    let frames = screen(&portal, &dir).capture().unwrap();
    assert_eq!(frames[0].image.width(), 4);
}

#[test]
fn a_session_without_streams_is_a_portal_error() {
    let dir = tempfile::tempdir().unwrap();
    let portal = Arc::new(FakePortal::with_streams(5, vec![]));
    assert_eq!(
        screen(&portal, &dir).capture().err(),
        Some(CaptureError::Portal)
    );
}

#[test]
fn the_token_store_ignores_junk() {
    let dir = tempfile::tempdir().unwrap();
    let store = TokenStore::new(dir.path().join("t"));
    assert_eq!(store.load(), None);
    std::fs::write(store.path(), "  \n").unwrap();
    assert_eq!(store.load(), None);
    std::fs::write(store.path(), vec![b'a'; 10_000]).unwrap();
    assert_eq!(store.load(), None);
    store.save("abc\n").unwrap();
    assert_eq!(store.load().as_deref(), Some("abc"));
}
