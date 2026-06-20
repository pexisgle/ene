//! X11 fallback for the click-through story.
//!
//! The X11 `shape` extension is the Linux-side analog of Wayland's
//! `set_input_region`. The taskbar and pager are suppressed via
//! `_NET_WM_STATE_SKIP_TASKBAR` / `_NET_WM_STATE_SKIP_PAGER`
//! (EWMH).
//!
//! PR5.4.1 will add the real `x11rb` integration and the shape
//! extension. This stub exists so the platform module is wired
//! through `AppState` and the build does not break in the
//! meantime.
use std::sync::Arc;

use parking_lot::Mutex;
use winit::raw_window_handle::{HasDisplayHandle, HasWindowHandle};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct X11WindowId(pub u32);

#[allow(dead_code)] // `window_id` is read by the real PR5.4.1 implementation.
pub struct X11Context {
    pub window_id: X11WindowId,
}

impl X11Context {
    /// Stub: always returns `None` until the real `x11rb`
    /// integration lands in PR5.4.1.
    #[allow(dead_code)] // Consumed by the runtime in PR5.4.1.
    pub fn try_new<W: HasWindowHandle + HasDisplayHandle>(_window: &W) -> Option<Arc<Mutex<Self>>> {
        None
    }

    /// Set `_NET_WM_STATE_SKIP_TASKBAR` and
    /// `_NET_WM_STATE_SKIP_PAGER`. No-op in the stub.
    #[allow(dead_code)] // Consumed by the runtime in PR5.4.1.
    pub fn set_skip_taskbar(&mut self, _skip: bool) {}

    /// Update the X11 shape extension input region. No-op in
    /// the stub.
    #[allow(dead_code)] // Consumed by the runtime in PR5.4.1.
    pub fn set_input_rects(&mut self, _rects: &[(i32, i32, i32, i32)]) {}
}
