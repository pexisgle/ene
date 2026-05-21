#[cfg(target_os = "linux")]
use ene_tool_proto::ToolError;

#[cfg(target_os = "linux")]
use image::{DynamicImage, imageops::FilterType};

#[cfg(target_os = "linux")]
pub fn detect_wayland() -> bool {
    std::env::var("XDG_SESSION_TYPE")
        .map(|s| s == "wayland")
        .unwrap_or(false)
        || std::env::var("WAYLAND_DISPLAY").is_ok()
}

#[cfg(not(target_os = "linux"))]
pub fn detect_wayland() -> bool {
    false
}

#[cfg(target_os = "linux")]
fn resize_image(image: DynamicImage, scale_percent: u32) -> DynamicImage {
    if scale_percent > 0 && scale_percent < 100 {
        let nwidth = (image.width() as f32 * (scale_percent as f32 / 100.0)) as u32;
        let nheight = (image.height() as f32 * (scale_percent as f32 / 100.0)) as u32;
        image.resize(nwidth.max(1), nheight.max(1), FilterType::Lanczos3)
    } else {
        image
    }
}

#[cfg(target_os = "linux")]
pub async fn capture_screen_portal(scale_percent: u32) -> Result<DynamicImage, ToolError> {
    use ashpd::desktop::screenshot::Screenshot;

    let response = Screenshot::request()
        .interactive(false)
        .modal(false)
        .send()
        .await
        .map_err(|e| ToolError::ExecutionFailed { message: format!("Portal screenshot request failed: {e}") })?
        .response()
        .map_err(|e| ToolError::ExecutionFailed { message: format!("Portal screenshot response failed: {e}") })?;

    let uri = response.uri();
    let path = uri.as_str().strip_prefix("file://").unwrap_or(uri.as_str());

    let image = image::open(path)
        .map_err(|e| ToolError::ExecutionFailed { message: format!("Failed to open screenshot file: {e}") })?;

    let _ = std::fs::remove_file(path);

    Ok(resize_image(image, scale_percent))
}

#[cfg(not(target_os = "linux"))]
pub async fn capture_screen_portal(_scale_percent: u32) -> Result<DynamicImage, ToolError> {
    Err(ToolError::ExecutionFailed { message: "Portal not available on this platform".to_string() })
}

#[cfg(target_os = "linux")]
pub async fn capture_window_portal(scale_percent: u32) -> Result<DynamicImage, ToolError> {
    use ashpd::desktop::screencast::{
        CursorMode, Screencast, SelectSourcesOptions, SourceType, StartCastOptions,
    };

    let proxy = Screencast::new()
        .await
        .map_err(|e| ToolError::ExecutionFailed { message: format!("Failed to create Screencast proxy: {e}") })?;

    let session = proxy
        .create_session(Default::default())
        .await
        .map_err(|e| ToolError::ExecutionFailed { message: format!("Failed to create screencast session: {e}") })?;

    let source_opts = SelectSourcesOptions::default()
        .set_cursor_mode(CursorMode::Hidden)
        .set_sources(Some(SourceType::Window.into()))
        .set_multiple(false);

    proxy
        .select_sources(&session, source_opts)
        .await
        .map_err(|e| ToolError::ExecutionFailed { message: format!("Failed to select sources: {e}") })?;

    let response = proxy
        .start(&session, None, StartCastOptions::default())
        .await
        .map_err(|e| ToolError::ExecutionFailed { message: format!("Failed to start screencast: {e}") })?
        .response()
        .map_err(|e| ToolError::ExecutionFailed { message: format!("Screencast start response failed: {e}") })?;

    let streams = response.streams();
    if streams.is_empty() {
        return Err(ToolError::ExecutionFailed { message: 
            "No PipeWire streams returned from screencast".to_string(),
         });
    }

    let node_id = streams[0].pipe_wire_node_id();

    tokio::task::spawn_blocking(move || capture_pipewire_frame(node_id))
        .await
        .map_err(|e| ToolError::ExecutionFailed { message: format!("PipeWire task failed: {e}") })?
        .map(|img| resize_image(img, scale_percent))
}

#[cfg(not(target_os = "linux"))]
pub async fn capture_window_portal(_scale_percent: u32) -> Result<DynamicImage, ToolError> {
    Err(ToolError::ExecutionFailed { message: "Portal not available on this platform".to_string() })
}

#[cfg(target_os = "linux")]
fn capture_pipewire_frame(node_id: u32) -> Result<DynamicImage, ToolError> {
    use pipewire as pw;
    use pw::properties::properties;
    use pw::spa;
    use spa::pod::Pod;
    use std::cell::RefCell;
    use std::rc::Rc;

    pw::init();

    let mainloop = pw::main_loop::MainLoopRc::new(None)
        .map_err(|e| ToolError::ExecutionFailed { message: format!("Failed to create PipeWire mainloop: {e}") })?;

    let context = pw::context::ContextRc::new(&mainloop, None)
        .map_err(|e| ToolError::ExecutionFailed { message: format!("Failed to create PipeWire context: {e}") })?;

    let core = context
        .connect_rc(None)
        .map_err(|e| ToolError::ExecutionFailed { message: format!("Failed to connect PipeWire core: {e}") })?;

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
    .map_err(|e| ToolError::ExecutionFailed { message: format!("Failed to create PipeWire stream: {e}") })?;

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

            let (media_type, media_subtype) =
                match spa::param::format_utils::parse_format(param) {
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
                spa::param::video::VideoFormat::RGB
                | spa::param::video::VideoFormat::BGR => 3,
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
                std::ptr::copy_nonoverlapping(
                    raw_data.as_ptr(),
                    pixels.as_mut_ptr(),
                    data_size,
                );
            }
            s.pixels = Some(pixels);

            if let Some(ml) = &s.mainloop {
                ml.quit();
            }
        })
        .register()
        .map_err(|e| ToolError::ExecutionFailed { message: format!("Failed to register stream listener: {e}") })?;

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
            spa::utils::Fraction {
                num: 240,
                denom: 1
            }
        ),
    );
    let values: Vec<u8> = spa::pod::serialize::PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &spa::pod::Value::Object(obj),
    )
    .map_err(|e| ToolError::ExecutionFailed { message: format!("Failed to serialize pod: {e}") })?
    .0
    .into_inner();

    let pod = Pod::from_bytes(&values)
        .ok_or_else(|| ToolError::ExecutionFailed { message: "Failed to parse pod from bytes".to_string() })?;
    let mut params = [pod];

    stream
        .connect(
            spa::utils::Direction::Input,
            Some(node_id),
            pw::stream::StreamFlags::AUTOCONNECT | pw::stream::StreamFlags::MAP_BUFFERS,
            &mut params,
        )
        .map_err(|e| ToolError::ExecutionFailed { message: format!("Failed to connect PipeWire stream: {e}") })?;

    mainloop.run();

    let s = state.borrow();
    let pixels = s
        .pixels
        .as_ref()
        .ok_or_else(|| ToolError::ExecutionFailed { message: "No frame captured from PipeWire".to_string() })?;
    let width = s.width;
    let height = s.height;
    let is_bgr = s.is_bgr;
    let bpp = s.bpp;

    let image = if is_bgr && bpp >= 4 {
        let mut rgba = pixels.clone();
        for chunk in rgba.chunks_exact_mut(bpp as usize) {
            chunk.swap(0, 2);
        }
        match bpp {
            4 => DynamicImage::ImageRgba8(
                image::RgbaImage::from_raw(width, height, rgba).ok_or_else(|| {
                    ToolError::ExecutionFailed { message: "Invalid image dimensions".to_string() }
                })?,
            ),
            _ => DynamicImage::ImageRgba8(
                image::RgbaImage::from_raw(width, height, rgba).ok_or_else(|| {
                    ToolError::ExecutionFailed { message: "Invalid image dimensions".to_string() }
                })?,
            ),
        }
    } else if is_bgr && bpp == 3 {
        let mut rgb = pixels.clone();
        for chunk in rgb.chunks_exact_mut(3) {
            chunk.swap(0, 2);
        }
        DynamicImage::ImageRgb8(
            image::RgbImage::from_raw(width, height, rgb).ok_or_else(|| {
                ToolError::ExecutionFailed { message: "Invalid image dimensions".to_string() }
            })?,
        )
    } else if bpp == 4 {
        DynamicImage::ImageRgba8(
            image::RgbaImage::from_raw(width, height, pixels.clone()).ok_or_else(|| {
                ToolError::ExecutionFailed { message: "Invalid image dimensions".to_string() }
            })?,
        )
    } else {
        DynamicImage::ImageRgb8(
            image::RgbImage::from_raw(width, height, pixels.clone()).ok_or_else(|| {
                ToolError::ExecutionFailed { message: "Invalid image dimensions".to_string() }
            })?,
        )
    };

    Ok(image)
}

// ==================== Window Listing & Focus (Wayland) ====================

#[cfg(target_os = "linux")]
fn detect_wl_compositor() -> Option<String> {
    if std::env::var("HYPRLAND_INSTANCE_SIGNATURE").is_ok() {
        return Some("hyprland".to_string());
    }
    if std::env::var("SWAYSOCK").is_ok() {
        return Some("sway".to_string());
    }
    if let Ok(desktop) = std::env::var("XDG_CURRENT_DESKTOP") {
        let d = desktop.to_lowercase();
        if d.contains("gnome") {
            return Some("gnome".to_string());
        }
        if d.contains("kde") || d.contains("plasma") {
            return Some("kde".to_string());
        }
    }
    if std::env::var("KDE_FULL_SESSION").is_ok() {
        return Some("kde".to_string());
    }
    None
}

#[cfg(target_os = "linux")]
fn unescape_gvariant(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') => result.push('\n'),
                Some('t') => result.push('\t'),
                Some('r') => result.push('\r'),
                Some('\\') => result.push('\\'),
                Some('\'') => result.push('\''),
                Some('"') => result.push('"'),
                Some(c2) => {
                    result.push('\\');
                    result.push(c2);
                }
                None => result.push('\\'),
            }
        } else {
            result.push(c);
        }
    }
    result
}

#[cfg(target_os = "linux")]
fn parse_gdbus_tuple_string(output: &str) -> Option<String> {
    let output = output.trim();
    let inner = output.strip_prefix('(')?.strip_suffix(')')?;
    let comma = inner.find(", '")?;
    if !inner[..comma].starts_with("true") {
        return None;
    }
    let content = &inner[comma + 3..];
    let content = content.strip_suffix('\'')?;
    Some(unescape_gvariant(content))
}

#[cfg(target_os = "linux")]
fn list_windows_hyprland() -> Result<String, ToolError> {
    let output = std::process::Command::new("hyprctl")
        .args(["clients", "-j"])
        .output()
        .map_err(|e| ToolError::ExecutionFailed { message: format!("Failed to run hyprctl: {e}") })?;

    if !output.status.success() {
        return Err(ToolError::ExecutionFailed { message: format!(
            "hyprctl failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ) });
    }

    let clients: Vec<serde_json::Value> = serde_json::from_slice(&output.stdout)
        .map_err(|e| ToolError::ExecutionFailed { message: format!("Failed to parse hyprctl JSON: {e}") })?;

    let mut windows = Vec::new();
    for client in &clients {
        let title = client["title"].as_str().unwrap_or("");
        let class = client["class"].as_str().unwrap_or("");
        if !title.is_empty() || !class.is_empty() {
            windows.push(format!("{} ({})", title, class));
        }
    }
    Ok(windows.join("\n"))
}

#[cfg(target_os = "linux")]
fn sway_find_windows(node: &serde_json::Value, windows: &mut Vec<String>) {
    let node_type = node["type"].as_str().unwrap_or("");
    if node_type == "con" || node_type == "floating_con" {
        let name = node["name"].as_str().unwrap_or("");
        let app_id = node["app_id"].as_str().unwrap_or("");
        let class = node["window_properties"]["class"]
            .as_str()
            .unwrap_or("");
        let has_window = node["window"].as_i64().is_some();

        if has_window || !name.is_empty() || !app_id.is_empty() || !class.is_empty() {
            let display_id = if !app_id.is_empty() {
                app_id
            } else {
                class
            };
            if !name.is_empty() || !display_id.is_empty() {
                windows.push(format!("{} ({})", name, display_id));
            }
        }
    }

    if let Some(nodes) = node["nodes"].as_array() {
        for child in nodes {
            sway_find_windows(child, windows);
        }
    }
    if let Some(floating) = node["floating_nodes"].as_array() {
        for child in floating {
            sway_find_windows(child, windows);
        }
    }
}

#[cfg(target_os = "linux")]
fn list_windows_sway() -> Result<String, ToolError> {
    let output = std::process::Command::new("swaymsg")
        .args(["-t", "get_tree"])
        .output()
        .map_err(|e| ToolError::ExecutionFailed { message: format!("Failed to run swaymsg: {e}") })?;

    if !output.status.success() {
        return Err(ToolError::ExecutionFailed { message: format!(
            "swaymsg failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ) });
    }

    let tree: serde_json::Value = serde_json::from_slice(&output.stdout)
        .map_err(|e| ToolError::ExecutionFailed { message: format!("Failed to parse sway tree JSON: {e}") })?;

    let mut windows = Vec::new();
    sway_find_windows(&tree, &mut windows);
    Ok(windows.join("\n"))
}

#[cfg(target_os = "linux")]
fn list_windows_gnome() -> Result<String, ToolError> {
    let js = "global.get_window_actors().map(function(a){var w=a.meta_window;return w.get_title()+' | '+w.get_wm_class()}).join('\\n')";

    let output = std::process::Command::new("gdbus")
        .args([
            "call",
            "--session",
            "--dest",
            "org.gnome.Shell",
            "--object-path",
            "/org/gnome/Shell",
            "--method",
            "org.gnome.Shell.Eval",
            js,
        ])
        .output()
        .map_err(|e| ToolError::ExecutionFailed { message: format!("Failed to run gdbus: {e}") })?;

    if !output.status.success() {
        return Err(ToolError::ExecutionFailed { message: format!(
            "gdbus failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ) });
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let result = parse_gdbus_tuple_string(&stdout)
        .ok_or_else(|| ToolError::ExecutionFailed { message: "Failed to parse gdbus output".to_string() })?;

    let mut windows = Vec::new();
    for line in result.lines() {
        let line = line.trim();
        if !line.is_empty() {
            windows.push(line.to_string());
        }
    }
    Ok(windows.join("\n"))
}

#[cfg(target_os = "linux")]
fn try_qdbus_eval(js: &str) -> Result<String, ToolError> {
    let output = std::process::Command::new("qdbus")
        .args(["org.kde.plasmashell", "/PlasmaShell", "evaluateScript", js])
        .output()
        .map_err(|e| ToolError::ExecutionFailed { message: format!("Failed to run qdbus: {e}") })?;

    if !output.status.success() {
        return Err(ToolError::ExecutionFailed { message: format!(
            "qdbus failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ) });
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

#[cfg(target_os = "linux")]
fn try_gdbus_kwin_eval(js: &str) -> Result<String, ToolError> {
    let output = std::process::Command::new("gdbus")
        .args([
            "call",
            "--session",
            "--dest",
            "org.kde.plasmashell",
            "--object-path",
            "/PlasmaShell",
            "--method",
            "org.kde.PlasmaShell.evaluateScript",
            js,
        ])
        .output()
        .map_err(|e| ToolError::ExecutionFailed { message: format!("Failed to run gdbus: {e}") })?;

    if !output.status.success() {
        return Err(ToolError::ExecutionFailed { message: format!(
            "gdbus failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ) });
    }

    parse_gdbus_tuple_string(&String::from_utf8_lossy(&output.stdout))
        .ok_or_else(|| ToolError::ExecutionFailed { message: "Failed to parse gdbus output".to_string() })
}

#[cfg(target_os = "linux")]
fn list_windows_kde() -> Result<String, ToolError> {
    let js = "workspace.clientList().map(function(c){return c.caption + ' | ' + (c.resourceClass || '')}).join('\\n')";

    if let Ok(result) = try_qdbus_eval(js) {
        if !result.is_empty() {
            return Ok(result);
        }
    }

    try_gdbus_kwin_eval(js)
}

#[cfg(target_os = "linux")]
fn focus_window_hyprland(title: &str) -> Result<String, ToolError> {
    let output = std::process::Command::new("hyprctl")
        .args(["dispatch", "focuswindow", &format!("title:{}", title)])
        .output()
        .map_err(|e| ToolError::ExecutionFailed { message: format!("Failed to run hyprctl: {e}") })?;

    if !output.status.success() {
        return Err(ToolError::ExecutionFailed { message: format!(
            "hyprctl focus failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ) });
    }

    Ok(format!("Focused window matching: {}", title))
}

#[cfg(target_os = "linux")]
fn focus_window_sway(title: &str) -> Result<String, ToolError> {
    let criteria = format!("[title=\"{}\"] focus", title);
    let output = std::process::Command::new("swaymsg")
        .arg(&criteria)
        .output()
        .map_err(|e| ToolError::ExecutionFailed { message: format!("Failed to run swaymsg: {e}") })?;

    if !output.status.success() {
        return Err(ToolError::ExecutionFailed { message: format!(
            "swaymsg focus failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ) });
    }

    Ok(format!("Focused window matching: {}", title))
}

#[cfg(target_os = "linux")]
fn focus_window_gnome(title: &str) -> Result<String, ToolError> {
    let escaped = title.replace('\\', "\\\\").replace('\'', "\\'");
    let js = format!(
        "global.get_window_actors().forEach(function(a){{var w=a.meta_window;if(w.get_title().indexOf('{}')!=-1||w.get_wm_class().indexOf('{}')!=-1){{w.activate(global.get_current_time());w.get_workspace().activate(global.get_current_time())}}}})",
        escaped, escaped
    );

    let output = std::process::Command::new("gdbus")
        .args([
            "call",
            "--session",
            "--dest",
            "org.gnome.Shell",
            "--object-path",
            "/org/gnome/Shell",
            "--method",
            "org.gnome.Shell.Eval",
            &js,
        ])
        .output()
        .map_err(|e| ToolError::ExecutionFailed { message: format!("Failed to run gdbus: {e}") })?;

    if !output.status.success() {
        return Err(ToolError::ExecutionFailed { message: format!(
            "gdbus focus failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ) });
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let result = parse_gdbus_tuple_string(&stdout)
        .ok_or_else(|| ToolError::ExecutionFailed { message: "Failed to parse gdbus output".to_string() })?;

    if result == "not found" || result.is_empty() {
        return Err(ToolError::ExecutionFailed { message: format!(
            "Window not found: {}",
            title
        ) });
    }

    Ok(format!("Focused window matching: {}", title))
}

#[cfg(target_os = "linux")]
fn focus_window_kde(title: &str) -> Result<String, ToolError> {
    let escaped = title.replace('\\', "\\\\").replace('\'', "\\'");
    let js = format!(
        "var cs=workspace.clientList();for(var i=0;i<cs.length;i++){{if(cs[i].caption.indexOf('{}')!=-1||(cs[i].resourceClass||'').indexOf('{}')!=-1){{workspace.activeWindow=cs[i];break}}}}",
        escaped, escaped
    );

    if let Ok(_result) = try_qdbus_eval(&js) {
        return Ok(format!("Focused window matching: {}", title));
    }

    let _ = try_gdbus_kwin_eval(&js)?;
    Ok(format!("Focused window matching: {}", title))
}

#[cfg(target_os = "linux")]
pub fn list_windows_wayland() -> Result<String, ToolError> {
    let compositor = detect_wl_compositor().unwrap_or_default();
    match compositor.as_str() {
        "hyprland" => list_windows_hyprland(),
        "sway" => list_windows_sway(),
        "gnome" => list_windows_gnome(),
        "kde" => list_windows_kde(),
        _ => Err(ToolError::ExecutionFailed { message: 
            "Window listing not supported on this Wayland compositor. Supported: Hyprland, Sway, GNOME, KDE.".to_string(),
         }),
    }
}

#[cfg(not(target_os = "linux"))]
pub fn list_windows_wayland() -> Result<String, ToolError> {
    Err(ToolError::ExecutionFailed { message: "Wayland not available on this platform".to_string() })
}

#[cfg(target_os = "linux")]
pub fn focus_window_wayland(title: &str) -> Result<String, ToolError> {
    let compositor = detect_wl_compositor().unwrap_or_default();
    match compositor.as_str() {
        "hyprland" => focus_window_hyprland(title),
        "sway" => focus_window_sway(title),
        "gnome" => focus_window_gnome(title),
        "kde" => focus_window_kde(title),
        _ => Err(ToolError::ExecutionFailed { message: 
            "Window focusing not supported on this Wayland compositor. Supported: Hyprland, Sway, GNOME, KDE.".to_string(),
         }),
    }
}

#[cfg(not(target_os = "linux"))]
pub fn focus_window_wayland(_title: &str) -> Result<String, ToolError> {
    Err(ToolError::ExecutionFailed { message: "Wayland not available on this platform".to_string() })
}
