//! Overlay OS hit-test backends. Slint and VRM never see these types.

#[cfg(all(target_os = "linux", not(target_os = "android")))]
mod wayland;
#[cfg(target_os = "windows")]
mod windows;
#[cfg(all(target_os = "linux", not(target_os = "android")))]
mod x11;

use std::time::{Duration, Instant};

use winit::window::Window;

use crate::interaction_controller::InteractionMode;
use crate::scene::{InteractionGeometry, PxRect, VisualGeometry};

const REGION_RATE_LIMIT: Duration = Duration::from_millis(125);
const REGION_THRESHOLD_PX: f32 = 4.0;

/// Best-effort overlay window hints (Linux layer-shell / input region).
///
/// winit 0.30 does not expose layer-shell; click-through uses
/// [`winit::window::Window::set_cursor_hittest`] on Windows and
/// [`OverlayPlatform`] on Linux.
pub fn apply_overlay_hints(_window: &winit::window::Window) {
    tracing::debug!("overlay platform hints applied");
}

/// Apply click-through to the native window when supported.
///
/// Production hit-test goes through [`OverlayPlatform`]. This helper is the
/// leftover HWND EXSTYLE path and is not used by the overlay.
pub fn apply_click_through(enabled: bool, hwnd: Option<isize>) {
    #[cfg(target_os = "windows")]
    {
        if apply_click_through_windows(enabled, hwnd) {
            return;
        }
    }
    let _ = hwnd;
    if enabled {
        tracing::warn!("click-through requested but not supported on this platform");
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
        tracing::warn!("click-through on Windows requires an HWND");
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
        tracing::warn!("SetWindowLongPtrW failed for click-through");
        return false;
    }
    true
}

/// Preferred persistent data directory for stage-local state.
#[must_use]
pub fn preferred_data_dir() -> std::path::PathBuf {
    ene_config::data_dir().join("stage")
}

/// OS backend attached to one overlay window.
#[derive(Default)]
pub struct OverlayPlatform {
    kind: PlatformKind,
    last_mode: Option<InteractionMode>,
    last_interaction: InteractionGeometry,
    last_apply: Option<Instant>,
}

#[derive(Default)]
enum PlatformKind {
    #[default]
    Unattached,
    #[cfg(target_os = "windows")]
    Windows,
    #[cfg(all(target_os = "linux", not(target_os = "android")))]
    Wayland(Box<wayland::WaylandRegion>),
    #[cfg(all(target_os = "linux", not(target_os = "android")))]
    X11(Box<x11::X11Shape>),
    Fallback,
}

impl OverlayPlatform {
    pub fn attach(window: &Window) -> Self {
        let kind = detect_backend(window);
        tracing::info!(backend = kind.name(), "overlay platform attached");
        Self {
            kind,
            last_mode: None,
            last_interaction: InteractionGeometry::default(),
            last_apply: None,
        }
    }

    #[must_use]
    pub fn name(&self) -> &'static str {
        self.kind.name()
    }

    /// Map controller mode + scene geometry onto the OS hit-test path.
    pub fn apply(
        &mut self,
        window: &Window,
        mode: InteractionMode,
        visual: &VisualGeometry,
        interaction: &InteractionGeometry,
    ) {
        let push = self.should_push_region(mode, interaction);
        match &mut self.kind {
            #[cfg(target_os = "windows")]
            PlatformKind::Windows => {
                windows::apply_mode(window, mode);
            }
            #[cfg(all(target_os = "linux", not(target_os = "android")))]
            PlatformKind::Wayland(backend) => {
                if push {
                    backend.apply(mode, interaction);
                }
            }
            #[cfg(all(target_os = "linux", not(target_os = "android")))]
            PlatformKind::X11(backend) => {
                if push {
                    backend.apply(mode, visual, interaction);
                }
            }
            PlatformKind::Unattached | PlatformKind::Fallback => {
                let _ = (window, visual, interaction);
                let enabled = !matches!(mode, InteractionMode::Passive);
                if self.last_mode != Some(mode) {
                    match window.set_cursor_hittest(enabled) {
                        Ok(()) => {}
                        Err(err) => tracing::debug!(error = %err, "cursor hittest unsupported"),
                    }
                }
            }
        }
        self.last_mode = Some(mode);
        self.last_interaction = interaction.clone();
        self.last_apply = Some(Instant::now());
    }

    fn should_push_region(&self, mode: InteractionMode, interaction: &InteractionGeometry) -> bool {
        if self.last_mode != Some(mode) {
            return true;
        }
        if let Some(last) = self.last_apply
            && last.elapsed() < REGION_RATE_LIMIT
            && interaction.within_threshold(&self.last_interaction, REGION_THRESHOLD_PX)
        {
            return false;
        }
        true
    }
}

impl PlatformKind {
    const fn name(&self) -> &'static str {
        match self {
            Self::Unattached => "unattached",
            #[cfg(target_os = "windows")]
            Self::Windows => "windows-dcomp",
            #[cfg(all(target_os = "linux", not(target_os = "android")))]
            Self::Wayland(_) => "wayland",
            #[cfg(all(target_os = "linux", not(target_os = "android")))]
            Self::X11(_) => "x11",
            Self::Fallback => "fallback",
        }
    }
}

fn detect_backend(window: &Window) -> PlatformKind {
    #[cfg(target_os = "windows")]
    {
        let _ = window;
        PlatformKind::Windows
    }
    #[cfg(all(target_os = "linux", not(target_os = "android")))]
    {
        if let Some(wayland) = wayland::WaylandRegion::try_new(window) {
            PlatformKind::Wayland(Box::new(wayland))
        } else if let Some(x11) = x11::X11Shape::try_new(window) {
            PlatformKind::X11(Box::new(x11))
        } else {
            PlatformKind::Fallback
        }
    }
    #[cfg(not(any(
        target_os = "windows",
        all(target_os = "linux", not(target_os = "android"))
    )))]
    {
        let _ = window;
        PlatformKind::Fallback
    }
}

pub(crate) fn rects_i32(rects: &[PxRect]) -> Vec<(i32, i32, i32, i32)> {
    rects
        .iter()
        .filter(|rect| !rect.is_empty())
        .map(|rect| {
            (
                rect.x.round() as i32,
                rect.y.round() as i32,
                rect.w.round().max(1.0) as i32,
                rect.h.round().max(1.0) as i32,
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_limit_skips_tiny_moves() {
        let mut platform = OverlayPlatform {
            kind: PlatformKind::Fallback,
            last_mode: Some(InteractionMode::Interactive),
            last_interaction: InteractionGeometry {
                rects: vec![PxRect::new(0.0, 0.0, 10.0, 10.0)],
            },
            last_apply: Some(Instant::now()),
        };
        let next = InteractionGeometry {
            rects: vec![PxRect::new(1.0, 0.0, 10.0, 10.0)],
        };
        assert!(!platform.should_push_region(InteractionMode::Interactive, &next));
        platform.last_apply = Instant::now()
            .checked_sub(Duration::from_millis(200))
            .or(Some(Instant::now()));
        assert!(platform.should_push_region(InteractionMode::Interactive, &next));
    }

    #[test]
    fn mode_change_bypasses_rate_limit() {
        let platform = OverlayPlatform {
            kind: PlatformKind::Fallback,
            last_mode: Some(InteractionMode::Passive),
            last_interaction: InteractionGeometry::default(),
            last_apply: Some(Instant::now()),
        };
        assert!(
            platform.should_push_region(InteractionMode::Dragging, &InteractionGeometry::default())
        );
    }
}
