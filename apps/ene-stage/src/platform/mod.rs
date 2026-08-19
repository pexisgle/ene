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
