//! Linux-only click-through dispatcher.
//!
//! Reads the latest [`CursorState`] + [`MaskCapture`] rects and
//! pushes them into [`WaylandInputRegion`] / [`X11ContextRes`].
//! This replaces the bottom half of the legacy
//! `update_char_window_cursor_and_hittest` (lines 1174–1182).
//!
//! On non-Linux builds the module is empty.
#[cfg(target_os = "linux")]
mod imp {
    use bevy_ecs::prelude::*;

    use crate::resource::cursor_state::CursorState;
    use crate::resource::platform_state::resources::{LastAppliedInputRects, LastInputSource};

    /// Forward the latest mask rectangles into the Wayland /
    /// X11 input-region state and write the `LastApplied*` /
    /// `LastInputSource` snapshot for the F9 debug overlay.
    ///
    /// Phase 6 keeps the implementation as a stub: the real
    /// per-frame rectangle push lands once Phase 7 wires
    /// `wgpu::Device` access into the bevy schedule. The
    /// `Last*` writes happen here so the F9 overlay stays in
    /// sync with the next frame's drawing.
    pub fn apply_linux_click_through_system(
        _cursor: Res<CursorState>,
        mut last_rects: ResMut<LastAppliedInputRects>,
        mut last_source: ResMut<LastInputSource>,
    ) {
        last_rects.0.clear();
        last_source.0 = crate::input_region_debug::InputRegionSource::Empty;
    }
}

#[cfg(target_os = "linux")]
pub use imp::apply_linux_click_through_system;

#[cfg(not(target_os = "linux"))]
pub fn apply_linux_click_through_system() {}
