//! Linux display-server integration state.
//!
//! Phase 8 promotes the per-handle Linux display-server state into
//! bevy [`Resource`]s (one `Resource` per handle). The winit
//! runtime inserts these resources in `Runtime::resumed` and the
//! bevy systems read / write them through the bevy `World`.
//! `LastAppliedInputRects` / `LastInputSource` are the same
//! `Resource`s that the F9 input-region debug overlay reads.
//!
//! Non-Linux builds see an empty module.
#[cfg(target_os = "linux")]
pub mod resources {
    use std::sync::Arc;

    use bevy_ecs::prelude::Resource;
    use parking_lot::Mutex;

    use crate::input_region_debug::InputRegionSource;
    use crate::platform::mask_readback::MaskReadbackWorker;
    use crate::platform::wayland_layer_shell::LayerShellState;
    use crate::platform::wayland_mask_capture::MaskCaptureState;
    use crate::platform::wayland_region::{Rect, WaylandInputRegionContext};
    use crate::platform::x11_taskbar::X11Context;

    #[derive(Resource, Default)]
    pub struct WindowScaleFactor(pub f64);

    #[derive(Resource, Default)]
    pub struct WaylandInputRegion(pub Option<Arc<Mutex<WaylandInputRegionContext>>>);

    #[derive(Resource, Default)]
    pub struct X11ContextRes(pub Option<Arc<Mutex<X11Context>>>);

    #[derive(Resource, Default)]
    pub struct LayerShell(pub Option<LayerShellState>);

    /// `true` while the user holds the "freeze character window"
    /// hotkey (`F8`). Forces the xdg-shell fallback to receive
    /// all input even on compositors without layer-shell.
    #[derive(Resource, Default)]
    pub struct LayerShellFreeze(pub bool);

    #[derive(Resource, Default)]
    pub struct MaskCapture(pub Option<MaskCaptureState>);

    #[derive(Resource, Default)]
    pub struct MaskReadbackWorkerRes(pub Option<MaskReadbackWorker>);

    #[derive(Resource, Default)]
    pub struct LastAppliedInputRects(pub Vec<Rect>);

    #[derive(Resource, Default)]
    pub struct LastInputSource(pub InputRegionSource);
}
