use ene_tool_proto::ToolError;
use image::DynamicImage;

pub(super) async fn capture_window_portal(scale_percent: u32) -> Result<DynamicImage, ToolError> {
    use ashpd::desktop::screencast::{
        CursorMode, Screencast, SelectSourcesOptions, SourceType, StartCastOptions,
    };

    let proxy = Screencast::new()
        .await
        .map_err(|e| ToolError::ExecutionFailed {
            message: format!("Failed to create Screencast proxy: {e}"),
        })?;

    let session = proxy
        .create_session(Default::default())
        .await
        .map_err(|e| ToolError::ExecutionFailed {
            message: format!("Failed to create screencast session: {e}"),
        })?;

    let source_opts = SelectSourcesOptions::default()
        .set_cursor_mode(CursorMode::Hidden)
        .set_sources(Some(SourceType::Window.into()))
        .set_multiple(false);

    proxy
        .select_sources(&session, source_opts)
        .await
        .map_err(|e| ToolError::ExecutionFailed {
            message: format!("Failed to select sources: {e}"),
        })?;

    let response = proxy
        .start(&session, None, StartCastOptions::default())
        .await
        .map_err(|e| ToolError::ExecutionFailed {
            message: format!("Failed to start screencast: {e}"),
        })?
        .response()
        .map_err(|e| ToolError::ExecutionFailed {
            message: format!("Screencast start response failed: {e}"),
        })?;

    let streams = response.streams();
    if streams.is_empty() {
        return Err(ToolError::ExecutionFailed {
            message: "No PipeWire streams returned from screencast".to_string(),
        });
    }

    let node_id = streams[0].pipe_wire_node_id();

    tokio::task::spawn_blocking(move || capture_pipewire_frame(node_id))
        .await
        .map_err(|e| ToolError::ExecutionFailed {
            message: format!("PipeWire task failed: {e}"),
        })?
        .map(|img| crate::utils::image::resize_image(img, scale_percent))
}

fn capture_pipewire_frame(node_id: u32) -> Result<DynamicImage, ToolError> {
    use pipewire as pw;
    use pw::properties::properties;
    use pw::spa;
    use spa::pod::Pod;
    use std::cell::RefCell;
    use std::rc::Rc;

    pw::init();

    let mainloop =
        pw::main_loop::MainLoopRc::new(None).map_err(|e| ToolError::ExecutionFailed {
            message: format!("Failed to create PipeWire mainloop: {e}"),
        })?;

    let context =
        pw::context::ContextRc::new(&mainloop, None).map_err(|e| ToolError::ExecutionFailed {
            message: format!("Failed to create PipeWire context: {e}"),
        })?;

    let core = context
        .connect_rc(None)
        .map_err(|e| ToolError::ExecutionFailed {
            message: format!("Failed to connect PipeWire core: {e}"),
        })?;

    struct CaptureState {
        pixels: Option<Vec<u8>>,
        width: u32,
        height: u32,
        stride: u32,
        bpp: u32,
        is_bgr: bool,
        mainloop: Option<pw::main_loop::MainLoopRc>,
    }

    let state = Rc::new(RefCell::new(CaptureState {
        pixels: None,
        width: 0,
        height: 0,
        stride: 0,
        bpp: 4,
        is_bgr: true,
        mainloop: Some(mainloop.clone()),
    }));

    let stream = pw::stream::StreamBox::new(
        &core,
        "ene-screencap",
        properties! {
            *pw::keys::MEDIA_TYPE => "Video",
            *pw::keys::MEDIA_CATEGORY => "Capture",
            *pw::keys::MEDIA_ROLE => "Screen",
        },
    )
    .map_err(|e| ToolError::ExecutionFailed {
        message: format!("Failed to create PipeWire stream: {e}"),
    })?;

    let state_for_listener = state.clone();
    let _listener = stream
        .add_local_listener_with_user_data(state_for_listener)
        .state_changed(|_stream, state, _old, new| {
            if let pw::stream::StreamState::Error(e) = &new {
                tracing::warn!("PipeWire stream error: {}", e);
                if let Some(ml) = &state.borrow().mainloop {
                    ml.quit();
                }
            }
        })
        .param_changed(|_stream, state, id, param| {
            let Some(param) = param else {
                return;
            };
            if id != spa::param::ParamType::Format.as_raw() {
                return;
            }

            let (media_type, media_subtype) = match spa::param::format_utils::parse_format(param) {
                Ok(v) => v,
                Err(_) => return,
            };

            if media_type != spa::param::format::MediaType::Video
                || media_subtype != spa::param::format::MediaSubtype::Raw
            {
                return;
            }

            let mut format: spa::param::video::VideoInfoRaw = Default::default();
            if format.parse(param).is_err() {
                return;
            }

            let fmt = format.format();
            let is_bgr = matches!(
                fmt,
                spa::param::video::VideoFormat::BGRx
                    | spa::param::video::VideoFormat::BGRA
                    | spa::param::video::VideoFormat::BGR
            );

            let bpp = match fmt {
                spa::param::video::VideoFormat::RGBA
                | spa::param::video::VideoFormat::BGRA
                | spa::param::video::VideoFormat::RGBx
                | spa::param::video::VideoFormat::BGRx => 4,
                spa::param::video::VideoFormat::RGB | spa::param::video::VideoFormat::BGR => 3,
                _ => 4,
            };

            let mut s = state.borrow_mut();
            s.bpp = bpp;
            s.is_bgr = is_bgr;
        })
        .process(|stream, state| {
            let mut buffer = match stream.dequeue_buffer() {
                Some(b) => b,
                None => return,
            };

            let datas = buffer.datas_mut();
            if datas.is_empty() {
                return;
            }

            let d = &mut datas[0];
            let chunk = d.chunk();
            let data_size = chunk.size() as usize;
            if data_size == 0 {
                return;
            }

            let stride = chunk.stride() as u32;

            let Some(raw_data) = d.data() else {
                return;
            };

            let mut s = state.borrow_mut();
            let pixel_stride = stride / s.bpp;
            s.stride = stride;
            s.width = pixel_stride;
            s.height = (data_size as u32) / stride;

            let mut pixels = vec![0u8; data_size];
            unsafe {
                std::ptr::copy_nonoverlapping(raw_data.as_ptr(), pixels.as_mut_ptr(), data_size);
            }
            s.pixels = Some(pixels);

            if let Some(ml) = &s.mainloop {
                ml.quit();
            }
        })
        .register()
        .map_err(|e| ToolError::ExecutionFailed {
            message: format!("Failed to register stream listener: {e}"),
        })?;

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
            spa::param::video::VideoFormat::BGRx,
            spa::param::video::VideoFormat::BGRx,
            spa::param::video::VideoFormat::RGBx,
            spa::param::video::VideoFormat::BGRA,
            spa::param::video::VideoFormat::RGBA,
            spa::param::video::VideoFormat::RGB,
            spa::param::video::VideoFormat::BGR
        ),
        spa::pod::property!(
            spa::param::format::FormatProperties::VideoSize,
            Choice,
            Range,
            Rectangle,
            spa::utils::Rectangle {
                width: 3840,
                height: 2160
            },
            spa::utils::Rectangle {
                width: 1,
                height: 1
            },
            spa::utils::Rectangle {
                width: 8192,
                height: 8192
            }
        ),
        spa::pod::property!(
            spa::param::format::FormatProperties::VideoFramerate,
            Choice,
            Range,
            Fraction,
            spa::utils::Fraction { num: 60, denom: 1 },
            spa::utils::Fraction { num: 0, denom: 1 },
            spa::utils::Fraction { num: 240, denom: 1 }
        ),
    );
    let values: Vec<u8> = spa::pod::serialize::PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &spa::pod::Value::Object(obj),
    )
    .map_err(|e| ToolError::ExecutionFailed {
        message: format!("Failed to serialize pod: {e}"),
    })?
    .0
    .into_inner();

    let pod = Pod::from_bytes(&values).ok_or_else(|| ToolError::ExecutionFailed {
        message: "Failed to parse pod from bytes".to_string(),
    })?;
    let mut params = [pod];

    stream
        .connect(
            spa::utils::Direction::Input,
            Some(node_id),
            pw::stream::StreamFlags::AUTOCONNECT | pw::stream::StreamFlags::MAP_BUFFERS,
            &mut params,
        )
        .map_err(|e| ToolError::ExecutionFailed {
            message: format!("Failed to connect PipeWire stream: {e}"),
        })?;

    mainloop.run();

    let s = state.borrow();
    let pixels = s
        .pixels
        .as_ref()
        .ok_or_else(|| ToolError::ExecutionFailed {
            message: "No frame captured from PipeWire".to_string(),
        })?;
    let width = s.width;
    let height = s.height;
    let is_bgr = s.is_bgr;
    let bpp = s.bpp;

    let image =
        if is_bgr && bpp >= 4 {
            let mut rgba = pixels.clone();
            for chunk in rgba.chunks_exact_mut(bpp as usize) {
                chunk.swap(0, 2);
            }
            match bpp {
                4 => DynamicImage::ImageRgba8(
                    image::RgbaImage::from_raw(width, height, rgba).ok_or_else(|| {
                        ToolError::ExecutionFailed {
                            message: "Invalid image dimensions".to_string(),
                        }
                    })?,
                ),
                _ => DynamicImage::ImageRgba8(
                    image::RgbaImage::from_raw(width, height, rgba).ok_or_else(|| {
                        ToolError::ExecutionFailed {
                            message: "Invalid image dimensions".to_string(),
                        }
                    })?,
                ),
            }
        } else if is_bgr && bpp == 3 {
            let mut rgb = pixels.clone();
            for chunk in rgb.chunks_exact_mut(3) {
                chunk.swap(0, 2);
            }
            DynamicImage::ImageRgb8(image::RgbImage::from_raw(width, height, rgb).ok_or_else(
                || ToolError::ExecutionFailed {
                    message: "Invalid image dimensions".to_string(),
                },
            )?)
        } else if bpp == 4 {
            DynamicImage::ImageRgba8(
                image::RgbaImage::from_raw(width, height, pixels.clone()).ok_or_else(|| {
                    ToolError::ExecutionFailed {
                        message: "Invalid image dimensions".to_string(),
                    }
                })?,
            )
        } else {
            DynamicImage::ImageRgb8(
                image::RgbImage::from_raw(width, height, pixels.clone()).ok_or_else(|| {
                    ToolError::ExecutionFailed {
                        message: "Invalid image dimensions".to_string(),
                    }
                })?,
            )
        };

    Ok(image)
}
