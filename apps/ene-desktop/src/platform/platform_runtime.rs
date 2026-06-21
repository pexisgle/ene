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
use crate::state::AppState;

#[cfg(target_os = "linux")]
use std::sync::atomic::{AtomicBool, Ordering};

#[cfg(target_os = "linux")]
static FIRST_DISPATCH_LOGGED: AtomicBool = AtomicBool::new(false);

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
#[cfg(target_os = "linux")]
pub fn apply_linux_click_through(
    state: &mut AppState,
    allows_input: bool,
    cursor_on_silhouette: bool,
    freeze_forced: bool,
) {
    let (rects, source) = if freeze_forced {
        (
            vec![(0_i32, 0_i32, i32::from(i16::MAX), i32::from(i16::MAX))],
            super::super::input_region_debug::InputRegionSource::Freeze,
        )
    } else if allows_input && (cursor_on_silhouette || state.character.drag.is_dragging()) {
        (
            vec![(0_i32, 0_i32, i32::from(i16::MAX), i32::from(i16::MAX))],
            super::super::input_region_debug::InputRegionSource::FullWindow,
        )
    } else if let Some(mask) = state.mask_capture.as_ref() {
        let guard = mask.lock();
        let extracted = guard.extract_rectangles();
        if extracted.is_empty() {
            (
                Vec::new(),
                super::super::input_region_debug::InputRegionSource::Empty,
            )
        } else {
            let factor = guard.downsample() as i64;
            let scaled: Vec<super::wayland_region::Rect> = extracted
                .into_iter()
                .map(|(x, y, w, h)| {
                    (
                        (i64::from(x) * factor).min(i64::from(i32::MAX)) as i32,
                        (i64::from(y) * factor).min(i64::from(i32::MAX)) as i32,
                        (i64::from(w) * factor).min(i64::from(i32::MAX)) as i32,
                        (i64::from(h) * factor).min(i64::from(i32::MAX)) as i32,
                    )
                })
                .collect();
            drop(guard);
            (
                scaled,
                super::super::input_region_debug::InputRegionSource::Mask,
            )
        }
    } else {
        (
            Vec::new(),
            super::super::input_region_debug::InputRegionSource::Empty,
        )
    };

    if let Some(ctx) = state.wayland_region.as_ref() {
        let mut guard = ctx.lock();
        guard.pump();

        if freeze_forced
            || (allows_input && (cursor_on_silhouette || state.character.drag.is_dragging()))
        {
            guard.set_full_input();
        } else {
            guard.set_rects(rects.clone());
        }

        guard.apply_to_winit_surface();
    }

    if let Some(ctx) = state.x11_ctx.as_ref() {
        let mut guard = ctx.lock();
        if rects.is_empty() {
            guard.clear_input();
        } else {
            guard.set_input_rects(&rects);
        }
    }

    if !FIRST_DISPATCH_LOGGED.swap(true, Ordering::Relaxed) {
        let layer_shell_cached = state
            .layer_shell
            .as_ref()
            .is_some_and(|ctx| ctx.lock().cached().is_some());
        let x11_path = if state.x11_ctx.is_some() {
            let path = super::x11_taskbar::X11Path::decide(
                allows_input,
                cursor_on_silhouette || state.character.drag.is_dragging(),
                freeze_forced,
            );
            Some(path)
        } else {
            None
        };

        tracing::trace!(
            target: "ene.linux.hit_test",
            allows_input,
            cursor_on_silhouette,
            freeze_forced,
            wayland = state.wayland_region.is_some(),
            x11 = state.x11_ctx.is_some(),
            x11_path = ?x11_path,
            layer_shell_cached,
            "char window hit test (linux) — first dispatch per process"
        );
    }

    state.last_applied_input_rects = rects.clone();
    state.last_input_source = source;
}

/// Run the layer-shell detection probe against the stand-alone
/// Wayland connection (if any) and update the cached status.
#[cfg(target_os = "linux")]
pub fn detect_layer_shell(state: &AppState) -> super::wayland_layer_shell::LayerShellStatus {
    use super::wayland_layer_shell::LayerShellStatus;
    let Some(layer_shell) = state.layer_shell.as_ref() else {
        return LayerShellStatus::Unavailable;
    };
    let connection_owned: Option<wayland_client::Connection> = state
        .wayland_region
        .as_ref()
        .and_then(|region| region.lock().clone_connection());
    let connection_ref: Option<&wayland_client::Connection> = connection_owned.as_ref();
    let mut ctx = layer_shell.lock();
    ctx.status(connection_ref)
}
