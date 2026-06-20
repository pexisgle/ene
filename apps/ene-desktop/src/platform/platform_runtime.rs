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
    state: &AppState,
    allows_input: bool,
    cursor_on_silhouette: bool,
    freeze_forced: bool,
) {
    if let Some(ctx) = state.wayland_region.as_ref() {
        let mut guard = ctx.lock();

        // Drain the stand-alone Wayland connection's event
        // queue so the `wl_compositor` `bind` callback fires
        // promptly after construction. Cheap when the socket
        // has no events.
        guard.pump();

        if freeze_forced || (allows_input && cursor_on_silhouette) {
            guard.set_full_input();
        } else {
            guard.clear();
        }

        // LX.2: also push the policy to a stand-alone
        // `wl_surface` so the `set_input_region` call is
        // exercised end-to-end. A follow-up will swap the
        // stand-alone surface for winit's own surface once
        // winit 0.30 exposes a way to recover the
        // `wl_surface` from the raw handle.
        if let Some(surface) = guard.create_stand_alone_surface() {
            guard.apply_to_surface(&surface);
        }
    }

    // PR-LX.5: X11 fallback. The shape extension input region
    // is the X11 analog of Wayland's `set_input_region`. The
    // full-window rectangle set is the "accept all input" state
    // and an empty set is "pass through to the desktop". A
    // future PR-LX.7 will derive the rectangle set from
    // `MaskCaptureCamera::extract_rectangles` so the cursor only
    // receives input when it sits over the silhouette.
    if let Some(ctx) = state.x11_ctx.as_ref() {
        let path =
            super::x11_taskbar::X11Path::decide(allows_input, cursor_on_silhouette, freeze_forced);
        let mut guard = ctx.lock();
        match path {
            super::x11_taskbar::X11Path::Full | super::x11_taskbar::X11Path::Frozen => {
                // Full window rect: the shape input region
                // accepts all input. We use `i16::MAX` as the
                // width / height (the rectangles helper
                // saturates to `u16::MAX`); the X server clamps
                // the rect to the window extent.
                guard.set_input_rects(&[(0, 0, i32::from(i16::MAX), i32::from(i16::MAX))]);
            }
            super::x11_taskbar::X11Path::Empty => {
                guard.clear_input();
            }
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
                cursor_on_silhouette,
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
