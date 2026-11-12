//! Grabbing one SHM frame per stream with pipewire-rs.
//!
//! One main loop and one connection (the portal's file descriptor) serve all
//! streams. Each stream connects to its node, asks for a raw video format and
//! converts the first non-empty buffer to RGBA straight from the mapped
//! memory, so no extra raw copy is made. A timer ends the loop when the time
//! is up. Only memory is used; nothing is written to disk.

use std::cell::RefCell;
use std::os::fd::OwnedFd;
use std::rc::Rc;
use std::time::Duration;

use pipewire as pw;
use pw::properties::properties;
use pw::spa;
use pw::spa::param::video::{VideoFormat, VideoInfoRaw};
use pw::spa::pod::Pod;
use timereel_image::RgbaImage;

use super::frame::{convert_to_rgba, PixelFormat, MAX_SIDE};
use super::PortalError;

/// Per-stream state shared with the callbacks.
struct StreamData {
    index: usize,
    format: VideoInfoRaw,
}

fn pixel_format(format: VideoFormat) -> Option<PixelFormat> {
    if format == VideoFormat::BGRx {
        Some(PixelFormat::Bgrx)
    } else if format == VideoFormat::RGBx {
        Some(PixelFormat::Rgbx)
    } else if format == VideoFormat::BGRA {
        Some(PixelFormat::Bgra)
    } else if format == VideoFormat::RGBA {
        Some(PixelFormat::Rgba)
    } else {
        None
    }
}

/// The format request: raw video in one of the four accepted pixel layouts.
fn format_params() -> Result<Vec<u8>, PortalError> {
    let obj = spa::pod::object!(
        spa::utils::SpaTypes::ObjectParamFormat,
        spa::param::ParamType::EnumFormat,
        spa::pod::property!(
            spa::param::format::FormatProperties::MediaType,
            Id,
            spa::param::format::MediaType::Video
        ),
        spa::pod::property!(
            spa::param::format::FormatProperties::MediaSubtype,
            Id,
            spa::param::format::MediaSubtype::Raw
        ),
        spa::pod::property!(
            spa::param::format::FormatProperties::VideoFormat,
            Choice,
            Enum,
            Id,
            VideoFormat::BGRx,
            VideoFormat::BGRx,
            VideoFormat::RGBx,
            VideoFormat::BGRA,
            VideoFormat::RGBA,
        ),
        spa::pod::property!(
            spa::param::format::FormatProperties::VideoSize,
            Choice,
            Range,
            Rectangle,
            spa::utils::Rectangle {
                width: 1920,
                height: 1080
            },
            spa::utils::Rectangle {
                width: 1,
                height: 1
            },
            spa::utils::Rectangle {
                width: MAX_SIDE,
                height: MAX_SIDE
            }
        ),
        spa::pod::property!(
            spa::param::format::FormatProperties::VideoFramerate,
            Choice,
            Range,
            Fraction,
            spa::utils::Fraction { num: 30, denom: 1 },
            spa::utils::Fraction { num: 0, denom: 1 },
            spa::utils::Fraction {
                num: 1000,
                denom: 1
            }
        ),
    );
    spa::pod::serialize::PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &spa::pod::Value::Object(obj),
    )
    .map(|(cursor, _)| cursor.into_inner())
    .map_err(|_| PortalError::Failed)
}

/// Grabs one frame from each of `nodes` over the PipeWire remote `fd` and
/// returns them in the same order.
///
/// # Errors
/// [`PortalError::Failed`] if PipeWire fails, a stream errors, a buffer cannot
/// be read, or `timeout` runs out before every stream has delivered a frame.
pub fn grab_frames(
    fd: OwnedFd,
    nodes: &[u32],
    timeout: Duration,
) -> Result<Vec<RgbaImage>, PortalError> {
    pw::init();
    let fail = |what: &'static str| {
        move |_e: pw::Error| {
            tracing::warn!(step = what, "PipeWire call failed");
            PortalError::Failed
        }
    };
    let mainloop = pw::main_loop::MainLoopRc::new(None).map_err(fail("main loop"))?;
    let context = pw::context::ContextRc::new(&mainloop, None).map_err(fail("context"))?;
    let core = context.connect_fd_rc(fd, None).map_err(fail("connect"))?;

    let params_bytes = format_params()?;
    let results: Rc<RefCell<Vec<Option<RgbaImage>>>> =
        Rc::new(RefCell::new(nodes.iter().map(|_| None).collect()));
    let broken = Rc::new(RefCell::new(false));

    let mut streams = Vec::with_capacity(nodes.len());
    for (index, node) in nodes.iter().enumerate() {
        let stream = pw::stream::StreamBox::new(
            &core,
            "timereel-capture",
            properties! {
                *pw::keys::MEDIA_TYPE => "Video",
                *pw::keys::MEDIA_CATEGORY => "Capture",
                *pw::keys::MEDIA_ROLE => "Screen",
            },
        )
        .map_err(fail("stream"))?;

        let results_cb = Rc::clone(&results);
        let broken_cb = Rc::clone(&broken);
        let loop_cb = mainloop.clone();
        let broken_state = Rc::clone(&broken);
        let loop_state = mainloop.clone();
        let listener = stream
            .add_local_listener_with_user_data(StreamData {
                index,
                format: VideoInfoRaw::default(),
            })
            .state_changed(move |_, _, _old, new| {
                if let pw::stream::StreamState::Error(_) = new {
                    tracing::warn!("a PipeWire stream reported an error");
                    *broken_state.borrow_mut() = true;
                    loop_state.quit();
                }
            })
            .param_changed(|_, data, id, param| {
                let Some(param) = param else { return };
                if id != spa::param::ParamType::Format.as_raw() {
                    return;
                }
                let Ok((media_type, media_subtype)) = spa::param::format_utils::parse_format(param)
                else {
                    return;
                };
                if media_type != spa::param::format::MediaType::Video
                    || media_subtype != spa::param::format::MediaSubtype::Raw
                {
                    return;
                }
                if data.format.parse(param).is_err() {
                    tracing::warn!("could not read the stream format");
                }
            })
            .process(move |stream, data| {
                // Only the first good buffer of a stream is used.
                if results_cb.borrow()[data.index].is_some() {
                    return;
                }
                let Some(mut buffer) = stream.dequeue_buffer() else {
                    return;
                };
                let size = data.format.size();
                let Some(format) = pixel_format(data.format.format()) else {
                    return;
                };
                let Some(plane) = buffer.datas_mut().first_mut() else {
                    return;
                };
                let chunk = plane.chunk();
                let (offset, used, stride) = (
                    chunk.offset() as usize,
                    chunk.size() as usize,
                    i64::from(chunk.stride()),
                );
                // The compositor may send an empty buffer first.
                if used == 0 {
                    return;
                }
                let Some(mapped) = plane.data() else {
                    // Not mapped (for example a DMA-BUF): nothing to read.
                    tracing::warn!("a PipeWire buffer is not readable memory");
                    *broken_cb.borrow_mut() = true;
                    loop_cb.quit();
                    return;
                };
                let Some(bytes) = mapped.get(offset..) else {
                    return;
                };
                match convert_to_rgba(format, size.width, size.height, stride, bytes) {
                    Ok(image) => {
                        results_cb.borrow_mut()[data.index] = Some(image);
                        if results_cb.borrow().iter().all(Option::is_some) {
                            loop_cb.quit();
                        }
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "a frame could not be converted");
                        *broken_cb.borrow_mut() = true;
                        loop_cb.quit();
                    }
                }
            })
            .register()
            .map_err(fail("listener"))?;

        let mut params = [Pod::from_bytes(&params_bytes).ok_or(PortalError::Failed)?];
        stream
            .connect(
                spa::utils::Direction::Input,
                Some(*node),
                pw::stream::StreamFlags::AUTOCONNECT | pw::stream::StreamFlags::MAP_BUFFERS,
                &mut params,
            )
            .map_err(fail("stream connect"))?;
        streams.push((stream, listener));
    }

    let loop_timer = mainloop.clone();
    let timer = mainloop.loop_().add_timer(move |_| loop_timer.quit());
    timer
        .update_timer(Some(timeout), None)
        .into_result()
        .map_err(|_| PortalError::Failed)?;

    mainloop.run();
    drop(streams);

    if *broken.borrow() {
        return Err(PortalError::Failed);
    }
    let mut out = Vec::with_capacity(nodes.len());
    for slot in results.borrow_mut().drain(..) {
        match slot {
            Some(image) => out.push(image),
            None => {
                tracing::warn!("timed out waiting for a frame");
                return Err(PortalError::Failed);
            }
        }
    }
    Ok(out)
}
