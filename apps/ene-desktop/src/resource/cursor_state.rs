//! Holds the per-frame window-local cursor position so the
//! `apply_linux_click_through_system` and the raycast hit-test
//! can both read it without touching `AppState`.
use bevy_ecs::prelude::Resource;
use winit::dpi::PhysicalPosition;

#[derive(Resource, Debug, Default, Clone, Copy)]
pub struct CursorState {
    /// Last observed physical cursor position, in window-local
    /// pixel space. `None` until the first `CursorMoved` event
    /// arrives.
    pub physical: Option<PhysicalPosition<f64>>,
}
