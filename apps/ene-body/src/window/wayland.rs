//! KDE Wayland overlay backend (provisional).
//!
//! Intended path: `wayland-client` + `zwlr_layer_shell_v1` + ARGB + input
//! region so transparent pixels click through. winit AlwaysOnTop is not a
//! Wayland substitute. XWayland success is not KDE Wayland evidence.
//!
//! This Cloud Agent has no KDE Wayland session. The backend is a compile
//! stub. Probe status: 未実施.

use super::OverlayProbe;

/// Returns [`OverlayProbe::NotRun`]. Does not create a layer-shell surface.
#[must_use]
pub fn kde_layer_shell_probe() -> OverlayProbe {
    OverlayProbe::NotRun
}
