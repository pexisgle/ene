//! Linux click-through dispatcher.
//!
//! On Windows the per-frame hit-test result is plumbed through
//! `winit::window::Window::set_cursor_hittest`, which toggles
//! `WS_EX_TRANSPARENT` on the HWND. On Linux that call is a
//! no-op, so the runtime routes the same hit-test result through
//! the display-server-specific path implemented in this module.
//!
//! # Wayland
//!
//! [`WaylandInputRegionContext`](super::wayland_region::WaylandInputRegionContext)
//! pushes the per-frame policy into the cached state and a
//! stand-alone `wl_surface` (created from the bound
//! `wl_compositor`) receives the matching `set_input_region`
//! call. The runtime also drains the stand-alone connection's
//! event queue here so the compositor `bind` callback lands
//! promptly after construction.
//!
//! [`LayerShellContext`](super::wayland_layer_shell::LayerShellContext)
//! is consulted once on the first dispatch to detect
//! `zwlr_layer_shell_v1`; the cached status is logged for
//! diagnostics.
//!
//! # X11
//!
//! The X11 `shape` extension sets the input mask; the runtime
//! also drives `_NET_WM_STATE_SKIP_TASKBAR` via
//! [`super::x11_taskbar::X11Context`].
//!
//! # Cadence
//!
//! The function is called every `about_to_wait`, mirroring the
//! Windows `set_cursor_hittest` cadence. A `trace` log is emitted
//! only on the first dispatch per process so the no-op path is
//! observable without flooding the trace output.

#[cfg(target_os = "linux")]
use std::sync::Arc;

#[cfg(target_os = "linux")]
use std::sync::atomic::{AtomicBool, Ordering};

#[cfg(target_os = "linux")]
use parking_lot::Mutex;

#[cfg(target_os = "linux")]
use crate::input_region_debug::InputRegionSource;
#[cfg(target_os = "linux")]
use crate::platform::wayland_region::Rect;
#[cfg(target_os = "linux")]
use crate::platform::{
    wayland_layer_shell::LayerShellState, wayland_mask_capture::MaskCaptureState,
    wayland_region::WaylandInputRegionContext, x11_taskbar::X11Context,
};

#[cfg(target_os = "linux")]
static FIRST_DISPATCH_LOGGED: AtomicBool = AtomicBool::new(false);

/// Inputs to [`apply_linux_click_through`].
///
/// Phase 8 lifts every field out of the legacy `state::PlatformState`
/// (which is now removed) and the `&mut AppState` borrow that
/// used to be required to read them. Callers either pass
/// `bevy_ecs::Res<…>` references or, in the non-bevy runtime
/// path, raw `&Option<Arc<Mutex<…>>>` references.
#[cfg(target_os = "linux")]
#[derive(Default)]
pub struct ClickThroughInputs<'a> {
    pub wayland: Option<&'a Arc<Mutex<WaylandInputRegionContext>>>,
    pub x11: Option<&'a Arc<Mutex<X11Context>>>,
    pub layer_shell: Option<&'a LayerShellState>,
    pub layer_shell_freeze: bool,
    pub mask_capture: Option<&'a MaskCaptureState>,
    pub drag_is_dragging: bool,
    pub scale_factor: f64,
}

/// Per-frame click-through update on Linux.
///
/// `allows_input == true` keeps the character window receiving
/// pointer events; `false` makes the rest of the desktop receive
/// the click. `cursor_on_silhouette` is the coarse "is the cursor
/// over the model?" signal; on Wayland it is used to drive
/// `set_input_region`; on X11 it controls the shape mask.
///
/// `freeze_forced` is `true` while the user is holding the
/// `F8` "freeze character window" hotkey. When set, the click
/// through policy is bypassed: the character window receives
/// all input regardless of the cursor position so the user
/// can interact with the model on xdg-shell compositors that
/// do not advertise `zwlr_layer_shell_v1`.
///
/// Returns the `(rects, source)` pair that the caller should
/// publish into the `LastAppliedInputRects` / `LastInputSource`
/// bevy `Resource`s for the F9 debug overlay.
#[cfg(target_os = "linux")]
pub fn apply_linux_click_through(
    inputs: ClickThroughInputs<'_>,
    allows_input: bool,
    cursor_on_silhouette: bool,
    freeze_forced: bool,
) -> (Vec<Rect>, InputRegionSource) {
    let ClickThroughInputs {
        wayland,
        x11,
        layer_shell: _layer_shell,
        layer_shell_freeze: _layer_shell_freeze,
        mask_capture,
        drag_is_dragging,
        scale_factor,
    } = inputs;

    let (rects, source) = if freeze_forced {
        (
            vec![Rect::new(0, 0, i32::from(i16::MAX), i32::from(i16::MAX))],
            InputRegionSource::Freeze,
        )
    } else if allows_input && (cursor_on_silhouette || drag_is_dragging) {
        (
            vec![Rect::new(0, 0, i32::from(i16::MAX), i32::from(i16::MAX))],
            InputRegionSource::FullWindow,
        )
    } else if let Some(mask) = mask_capture {
        let guard = mask.lock();
        let extracted = guard.extract_rectangles();
        if extracted.is_empty() {
            (Vec::new(), InputRegionSource::Empty)
        } else {
            let factor = guard.downsample() as i64;
            let scaled: Vec<Rect> = extracted
                .into_iter()
                .map(|(x, y, w, h)| Rect {
                    x: (i64::from(x) * factor).min(i64::from(i32::MAX)) as i32,
                    y: (i64::from(y) * factor).min(i64::from(i32::MAX)) as i32,
                    w: (i64::from(w) * factor).min(i64::from(i32::MAX)) as i32,
                    h: (i64::from(h) * factor).min(i64::from(i32::MAX)) as i32,
                })
                .collect();
            drop(guard);
            (scaled, InputRegionSource::Mask)
        }
    } else {
        (Vec::new(), InputRegionSource::Empty)
    };

    if let Some(ctx) = wayland {
        let mut guard = ctx.lock();
        guard.pump();

        if freeze_forced || (allows_input && (cursor_on_silhouette || drag_is_dragging)) {
            guard.set_full_input();
        } else {
            let scale = scale_factor;
            let logical_rects: Vec<Rect> = rects
                .iter()
                .map(|r| Rect {
                    x: (r.x as f64 / scale).round() as i32,
                    y: (r.y as f64 / scale).round() as i32,
                    w: (r.w as f64 / scale).round().max(1.0) as i32,
                    h: (r.h as f64 / scale).round().max(1.0) as i32,
                })
                .collect();
            guard.set_rects(logical_rects);
        }

        guard.apply_to_winit_surface();
    }

    if let Some(ctx) = x11 {
        let mut guard = ctx.lock();
        if rects.is_empty() {
            guard.clear_input();
        } else {
            guard.set_input_rects(&rects);
        }
    }

    if !FIRST_DISPATCH_LOGGED.swap(true, Ordering::Relaxed) {
        let layer_shell_cached = _layer_shell.is_some_and(|ctx| ctx.lock().cached().is_some());
        let x11_path = if x11.is_some() {
            Some(super::x11_taskbar::X11Path::decide(
                allows_input,
                cursor_on_silhouette || drag_is_dragging,
                freeze_forced,
            ))
        } else {
            None
        };

        tracing::trace!(
            target: "ene.linux.hit_test",
            allows_input,
            cursor_on_silhouette,
            freeze_forced,
            wayland = wayland.is_some(),
            x11 = x11.is_some(),
            x11_path = ?x11_path,
            layer_shell_cached,
            "char window hit test (linux) — first dispatch per process"
        );
    }

    (rects, source)
}

/// Run the layer-shell detection probe against the stand-alone
/// Wayland connection (if any) and update the cached status.
#[cfg(target_os = "linux")]
pub fn detect_layer_shell(
    layer_shell: Option<&LayerShellState>,
    wayland: Option<&Arc<Mutex<WaylandInputRegionContext>>>,
) -> super::wayland_layer_shell::LayerShellStatus {
    use super::wayland_layer_shell::LayerShellStatus;
    let Some(layer_shell) = layer_shell else {
        return LayerShellStatus::Unavailable;
    };
    let connection_owned: Option<wayland_client::Connection> =
        wayland.and_then(|region| region.lock().clone_connection());
    let connection_ref: Option<&wayland_client::Connection> = connection_owned.as_ref();
    let mut ctx = layer_shell.lock();
    ctx.status(connection_ref)
}
