//! Linux click-through dispatcher (wgpu-migration §4 PR5.3 + §5 PR5.4).
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
//! diagnostics. A follow-up PR uses the status to promote the
//! character window to a `Layer::Overlay` surface when the
//! compositor supports it.
//!
//! # X11
//!
//! The X11 `shape` extension is wired in PR5.4.1. Until then the
//! `x11_ctx` field is `None` and the dispatch is a no-op.
//!
//! # Cadence
//!
//! The function is called every `about_to_wait`, mirroring the
//! Windows `set_cursor_hittest` cadence. A `trace` log is emitted
//! only on the **first** dispatch per process so the silent
//! no-op we used to see is no longer silent but the log does
//! not flood the trace output.

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
    // 1. Calculate the actual rectangles + source first so all display server
    //    backends (Wayland and X11) and the debug overlay receive the same data.
    let (rects, source) = if freeze_forced {
        (
            vec![(0_i32, 0_i32, i32::from(i16::MAX), i32::from(i16::MAX))],
            super::super::input_region_debug::InputRegionSource::Freeze,
        )
    } else if allows_input && (cursor_on_silhouette || state.character.drag.is_dragging()) {
        // The per-bone Rapier raycast said "yes"; accept
        // all input so the drag state machine receives the
        // events. The mask readback may be a frame behind
        // (one-frame latency) and at downsample=8 the
        // silhouette is only an approximation; falling back
        // to the rapier hit-test is more responsive.
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
            // PR-LX.8: scale downsampled-space rects to
            // window-pixel space before sending them to
            // the OS. `downsample` is always >= 1 (clamped
            // at construction), and we use saturating math
            // so an out-of-range downsampled rect cannot
            // overflow when multiplied.
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

    // 2. Set the rectangles on the Wayland context and apply to the surface.
    //    This must happen AFTER rects calculation so we do not overwrite the
    //    cached rectangles before they reach the compositor.
    if let Some(ctx) = state.wayland_region.as_ref() {
        let mut guard = ctx.lock();

        // Drain the stand-alone Wayland connection's event
        // queue so the `wl_compositor` `bind` callback fires
        // promptly after construction. Cheap when the socket
        // has no events.
        guard.pump();

        if freeze_forced
            || (allows_input && (cursor_on_silhouette || state.character.drag.is_dragging()))
        {
            guard.set_full_input();
        } else {
            guard.set_rects(rects.clone());
        }

        // Apply directly to winit's adopted wl_surface!
        guard.apply_to_winit_surface();
    }

    // PR-LX.5: X11 fallback. The shape extension input region
    // is the X11 analog of Wayland's `set_input_region`. The
    // rectangle set comes from the mask capture (above); the
    // empty set is "pass through to the desktop".
    if let Some(ctx) = state.x11_ctx.as_ref() {
        let mut guard = ctx.lock();
        if rects.is_empty() {
            guard.clear_input();
        } else {
            guard.set_input_rects(&rects);
        }
    }

    if !FIRST_DISPATCH_LOGGED.swap(true, Ordering::Relaxed) {
        // PR-LX.4: report layer-shell probe presence on the
        // first dispatch so the log carries the result. The
        // detection itself runs eagerly in `Runtime::resumed`;
        // this branch only reports whether the cache has
        // been populated.
        // PR-LX.5: report the X11 path taken on the first
        // dispatch so the log carries the result. The
        // connection itself is opened in `Runtime::resumed`;
        // this branch only reports which display-server path
        // the dispatcher took.
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

    // PR-LX.8: stash the final rectangle set + source on
    // the state so the F9 debug overlay can render them.
    // Done at the very end of the dispatcher (after all
    // OS-side pushes succeed) so the debug view mirrors
    // what was actually pushed to the display server.
    state.last_applied_input_rects = rects.clone();
    state.last_input_source = source;
}

/// Run the layer-shell detection probe against the stand-alone
/// Wayland connection (if any) and update the cached status.
/// Called once by the runtime after `resumed` so the first
/// `apply_linux_click_through` log can carry the result.
///
/// Returns the resolved [`LayerShellStatus`](super::wayland_layer_shell::LayerShellStatus)
/// so the caller can stash it for follow-up work.
#[cfg(target_os = "linux")]
pub fn detect_layer_shell(state: &AppState) -> super::wayland_layer_shell::LayerShellStatus {
    use super::wayland_layer_shell::LayerShellStatus;
    let Some(layer_shell) = state.layer_shell.as_ref() else {
        return LayerShellStatus::Unavailable;
    };
    // Clone the `Connection` out of the wayland_region
    // guard so the resulting `&Connection` outlives the
    // guard and can be passed across the layer-shell lock
    // acquisition.
    let connection_owned: Option<wayland_client::Connection> = state
        .wayland_region
        .as_ref()
        .and_then(|region| region.lock().clone_connection());
    let connection_ref: Option<&wayland_client::Connection> = connection_owned.as_ref();
    let mut ctx = layer_shell.lock();
    ctx.status(connection_ref)
}
