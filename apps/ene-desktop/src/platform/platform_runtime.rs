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
#[cfg(target_os = "linux")]
pub fn apply_linux_click_through(state: &AppState, allows_input: bool, cursor_on_silhouette: bool) {
    if let Some(ctx) = state.wayland_region.as_ref() {
        let mut guard = ctx.lock();

        // Drain the stand-alone Wayland connection's event
        // queue so the `wl_compositor` `bind` callback fires
        // promptly after construction. Cheap when the socket
        // has no events.
        guard.pump();

        if allows_input && cursor_on_silhouette {
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

    if !FIRST_DISPATCH_LOGGED.swap(true, Ordering::Relaxed) {
        tracing::trace!(
            target: "ene.linux.hit_test",
            allows_input,
            cursor_on_silhouette,
            wayland = state.wayland_region.is_some(),
            x11 = state.x11_ctx.is_some(),
            "char window hit test (linux) — first dispatch per process"
        );
    }
}
