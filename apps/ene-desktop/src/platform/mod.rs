//! Linux display-server integration (wgpu-migration §4 PR5.3 + §4 PR5.4).
//!
//! Winit 0.30's `Window::set_cursor_hittest` is a Windows-only
//! no-op on Linux, so the click-through story that PR5.2 shipped
//! for the Windows desktop (via `WS_EX_TRANSPARENT`) needs a
//! per-display-server implementation on Linux:
//!
//! - **Wayland** — `wl_surface::set_input_region` with a
//!   silhouette-derived rectangle set (PR5.3). When the compositor
//!   advertises `zwlr-layer-shell-v1` we additionally negotiate a
//!   `Layer::Overlay` surface so per-pixel alpha is composited
//!   above fullscreen windows (PR5.3 layer-shell).
//! - **X11** — `shape` extension input region +
//!   `_NET_WM_STATE_SKIP_TASKBAR` / `_SKIP_PAGER` so the desktop
//!   app does not appear in the task switcher (PR5.4).
//!
//! Submodules are gated individually so a build that only links
//! Wayland (e.g. a CI runner without X11) can still be exercised.
//! The Windows build does not link any of these modules.
#[cfg(target_os = "linux")]
pub mod mask_readback;
#[cfg(target_os = "linux")]
pub mod platform_runtime;
#[cfg(target_os = "linux")]
pub mod wayland_layer_shell;
#[cfg(target_os = "linux")]
pub mod wayland_mask_capture;
#[cfg(target_os = "linux")]
pub mod wayland_region;
#[cfg(target_os = "linux")]
pub mod x11_taskbar;

#[cfg(target_os = "linux")]
pub use platform_runtime::{apply_linux_click_through, detect_layer_shell};
