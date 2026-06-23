//! Linux display-server integration state.
//!
//! Phase 6 splits the legacy `state::PlatformState` struct into:
//!
//! 1. Per-handle [`Resource`]s inserted by `Runtime::resumed`
//!    once the winit window exists (e.g. `WaylandInputRegion`,
//!    `X11Context`, `LayerShellState`).
//! 2. A small `PlatformAdapters` struct that holds the handles
//!    the winit runtime must keep alive but that bevy does not
//!    own (e.g. the `MaskReadbackWorker` join handle, which is
//!    tied to the lifetime of the `wgpu::Device`).
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
    pub struct WaylandInputRegion(
        #[expect(dead_code, reason = "Populated by Runtime::resumed in Phase 7")]
        pub  Option<Arc<Mutex<WaylandInputRegionContext>>>,
    );

    #[derive(Resource, Default)]
    pub struct X11ContextRes(
        #[expect(dead_code, reason = "Populated by Runtime::resumed in Phase 7")]
        pub  Option<Arc<Mutex<X11Context>>>,
    );

    #[derive(Resource, Default)]
    #[expect(dead_code, reason = "Populated by Runtime::resumed in Phase 7")]
    pub struct LayerShell(pub Option<LayerShellState>);

    /// `true` while the user holds the "freeze character window"
    /// hotkey (`F8`). Forces the xdg-shell fallback to receive
    /// all input even on compositors without layer-shell.
    #[derive(Resource, Default)]
    #[expect(dead_code, reason = "Populated by Runtime::resumed in Phase 7")]
    pub struct LayerShellFreeze(pub bool);

    #[derive(Resource, Default)]
    #[expect(dead_code, reason = "Populated by Runtime::resumed in Phase 7")]
    pub struct MaskCapture(pub Option<MaskCaptureState>);

    #[derive(Resource, Default)]
    #[expect(dead_code, reason = "Populated by Runtime::resumed in Phase 7")]
    pub struct MaskReadbackWorkerRes(pub Option<MaskReadbackWorker>);

    #[derive(Resource, Default)]
    pub struct LastAppliedInputRects(pub Vec<Rect>);

    #[derive(Resource, Default)]
    pub struct LastInputSource(pub InputRegionSource);
}

/// Linux-only handles that the winit runtime must keep alive
/// but that bevy does not own. Held on `AppState::platform`.
#[cfg(target_os = "linux")]
#[derive(Default)]
#[expect(dead_code, reason = "Phase 6 placeholder; populated by Phase 7")]
pub struct PlatformAdapters {
    pub wayland_region: Option<
        std::sync::Arc<
            parking_lot::Mutex<crate::platform::wayland_region::WaylandInputRegionContext>,
        >,
    >,
    pub x11_ctx:
        Option<std::sync::Arc<parking_lot::Mutex<crate::platform::x11_taskbar::X11Context>>>,
    pub layer_shell: Option<crate::platform::wayland_layer_shell::LayerShellState>,
    pub layer_shell_freeze: bool,
    pub last_applied_input_rects: Vec<crate::platform::wayland_region::Rect>,
    pub last_input_source: crate::input_region_debug::InputRegionSource,
    pub mask_capture: Option<crate::platform::wayland_mask_capture::MaskCaptureState>,
    pub mask_readback_worker: Option<crate::platform::mask_readback::MaskReadbackWorker>,
}

/// Empty struct on non-Linux builds so `state.rs` doesn't
/// need `#[cfg]` at every access.
#[cfg(not(target_os = "linux"))]
#[derive(Default)]
pub struct PlatformAdapters;
