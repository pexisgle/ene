//! Platform-specific helpers for the stage overlay window.

use std::path::PathBuf;

use ene_config::data_dir;
use tracing::warn;

/// Best-effort overlay window hints (Linux layer-shell / input region).
///
/// winit 0.30 does not expose layer-shell; click-through uses
/// [`winit::window::Window::set_cursor_hittest`]. A compositor-specific
/// mask can be added later without changing the core contract.
pub fn apply_overlay_hints(_window: &winit::window::Window) {
    tracing::debug!("overlay platform hints applied (portable click-through only)");
}

/// Apply click-through to the native window when supported.
///
/// `hwnd` is optional on Windows (`HWND` as `isize`). When absent, the call is a no-op.
pub fn apply_click_through(enabled: bool, hwnd: Option<isize>) {
    #[cfg(target_os = "windows")]
    {
        if apply_click_through_windows(enabled, hwnd) {
            return;
        }
    }
    let _ = hwnd;
    if enabled {
        warn!("click-through requested but not supported on this platform");
    }
}

/// Returns the global pointer position when the platform exposes a native
/// query. Linux uses the existing X11 polling channel instead.
#[must_use]
pub fn global_cursor_position() -> Option<[i32; 2]> {
    #[cfg(target_os = "windows")]
    {
        use windows_sys::Win32::Foundation::POINT;
        use windows_sys::Win32::UI::WindowsAndMessaging::GetCursorPos;

        let mut point = POINT { x: 0, y: 0 };
        // SAFETY: `point` is a valid writable POINT for the duration of the
        // synchronous system call.
        if unsafe { GetCursorPos(&raw mut point) } != 0 {
            return Some([point.x, point.y]);
        }
    }
    None
}

#[cfg(target_os = "windows")]
fn apply_click_through_windows(enabled: bool, hwnd: Option<isize>) -> bool {
    use windows_sys::Win32::Foundation::HWND;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GWL_EXSTYLE, GetWindowLongPtrW, SetWindowLongPtrW, WS_EX_LAYERED, WS_EX_TRANSPARENT,
    };

    let Some(raw) = hwnd else {
        warn!("click-through on Windows requires an HWND");
        return false;
    };
    let hwnd = raw as HWND;
    // SAFETY: `hwnd` must refer to a valid top-level window for the duration of this call.
    let style = unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) };
    let next = if enabled {
        style | (WS_EX_LAYERED as isize) | (WS_EX_TRANSPARENT as isize)
    } else {
        style & !(WS_EX_TRANSPARENT as isize)
    };
    // SAFETY: same HWND validity as above; only EXSTYLE bits are toggled.
    let updated = unsafe { SetWindowLongPtrW(hwnd, GWL_EXSTYLE, next) };
    if updated == 0 {
        warn!("SetWindowLongPtrW failed for click-through");
        return false;
    }
    true
}

/// Preferred persistent data directory for stage-local state.
#[must_use]
pub fn preferred_data_dir() -> PathBuf {
    data_dir().join("stage")
}
