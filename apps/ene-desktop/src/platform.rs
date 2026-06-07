use bevy::camera::visibility::RenderLayers;
use bevy::prelude::Vec2;
#[cfg(target_os = "windows")]
use bevy::window::RawHandleWrapper;
use bevy::window::Window;
#[cfg(target_os = "windows")]
use raw_window_handle::RawWindowHandle;
#[cfg(target_os = "windows")]
use windows_sys::Win32::{
    Foundation::{HWND, POINT},
    Graphics::Gdi::ScreenToClient,
    UI::WindowsAndMessaging::GetCursorPos,
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LinuxWindowBackend {
    #[default]
    Direct,
    Wayland,
}

/// The layers used for the visible character and the offscreen silhouette camera.
pub fn character_render_layers() -> RenderLayers {
    RenderLayers::layer(0).with(1)
}

/// Detects whether the current Linux desktop session is Wayland-backed.
pub fn detect_linux_window_backend() -> LinuxWindowBackend {
    #[cfg(target_os = "linux")]
    {
        let wayland_display = std::env::var_os("WAYLAND_DISPLAY").is_some();
        let session_type_is_wayland = std::env::var("XDG_SESSION_TYPE")
            .is_ok_and(|value| value.eq_ignore_ascii_case("wayland"));

        if wayland_display || session_type_is_wayland {
            return LinuxWindowBackend::Wayland;
        }
    }

    LinuxWindowBackend::Direct
}

#[cfg(target_os = "windows")]
pub fn cursor_position_for_window(
    window: &Window,
    raw_handle: Option<&RawHandleWrapper>,
) -> Option<Vec2> {
    let hwnd = raw_handle.and_then(windows_hwnd)?;
    let mut point = POINT { x: 0, y: 0 };
    if unsafe { GetCursorPos(&mut point) } == 0 {
        return None;
    }
    if unsafe { ScreenToClient(hwnd, &mut point) } == 0 {
        return None;
    }

    let scale_factor = window.scale_factor();
    Some(Vec2::new(
        point.x as f32 / scale_factor,
        point.y as f32 / scale_factor,
    ))
}

#[cfg(target_os = "windows")]
pub fn windows_hwnd(raw_handle: &RawHandleWrapper) -> Option<HWND> {
    match raw_handle.get_window_handle() {
        RawWindowHandle::Win32(handle) => Some(handle.hwnd.get() as HWND),
        _ => None,
    }
}

#[cfg(not(target_os = "windows"))]
pub fn cursor_position_for_window(window: &Window) -> Option<Vec2> {
    window.cursor_position()
}
