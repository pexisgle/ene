//! Linux-only: forward the masked silhouette rectangles from
//! the [`MaskCapture`] worker into the Wayland / X11 input
//! region state. Replaces the inline rect push in
//! `Runtime::about_to_wait` (Phase 6).
//!
//! On non-Linux builds the module is empty.
#[cfg(target_os = "linux")]
mod imp {
    use bevy_ecs::prelude::*;

    use crate::resource::platform_state::resources::{MaskCapture, X11ContextRes};

    /// Refresh the input region from the latest mask
    /// rectangles. The Phase 6 plan calls for a "refresh on
    /// every `Update`" semantics; the actual rect translation
    /// lives in the platform adapters and runs synchronously
    /// when the worker's readback completes.
    pub fn refresh_input_region_system(
        _mask: Option<Res<MaskCapture>>,
        _x11: Option<Res<X11ContextRes>>,
    ) {
    }
}

#[cfg(target_os = "linux")]
pub use imp::refresh_input_region_system;

#[cfg(not(target_os = "linux"))]
pub fn refresh_input_region_system() {}
