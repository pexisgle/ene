//! Cursor state resource.
//!
//! Replaces the `Runtime::last_cursor_physical` field that the
//! winit event handler wrote and the per-frame hit-test read.
//! Phase 6 lifts it into a bevy `Resource` so the
//! `apply_linux_click_through_system` and the raycast hit-test
//! system can both read it without touching the legacy
//! `AppState`.
//!
//! The `update_cursor_state_system` (in
//! `system/platform/cursor.rs`) writes this from the
//! `PointerMoved` message stream on every frame where a cursor
//! move was observed.
use bevy_ecs::prelude::Resource;
use winit::dpi::PhysicalPosition;

#[derive(Resource, Debug, Default, Clone, Copy)]
pub struct CursorState {
    /// Last observed physical cursor position, in window-local
    /// pixel space. `None` until the first `CursorMoved` event
    /// arrives.
    pub physical: Option<PhysicalPosition<f64>>,
}
