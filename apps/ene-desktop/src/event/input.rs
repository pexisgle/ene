//! Input messages emitted by the winit event pump.
//!
//! These are independent of the legacy `AppEvent` bus. The
//! `pump_window_events` system (added in Phase 3) writes them; the
//! input / drag / look-at systems consume them.
use bevy_ecs::prelude::*;

/// Logical pointer move inside the character window's coordinate
/// space. `logical` is the post-DPI position passed to winit.
#[derive(Message, Debug, Clone, Copy)]
pub struct PointerMoved {
    #[expect(
        dead_code,
        reason = "Phase 8: device_query is the cursor source of truth; PointerMoved is reserved for a future migration"
    )]
    pub logical_x: f32,
    #[expect(
        dead_code,
        reason = "Phase 8: device_query is the cursor source of truth; PointerMoved is reserved for a future migration"
    )]
    pub logical_y: f32,
}

/// Mouse button state transition.
#[derive(Message, Debug, Clone, Copy)]
#[expect(dead_code, reason = "Consumed by input systems in Phase 3+")]
pub struct PointerButton {
    pub button: PointerButtonKind,
    pub pressed: bool,
}

/// Mouse button variants matching the winit names we care about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[expect(dead_code, reason = "Consumed by input systems in Phase 3+")]
pub enum PointerButtonKind {
    Left,
    Right,
    Middle,
    Other,
}

/// Key press or release, logical key name only.
#[derive(Message, Debug, Clone, Copy)]
#[expect(dead_code, reason = "Consumed by input systems in Phase 3+")]
pub struct KeyboardKey {
    pub named: Option<NamedKey>,
    pub code: Option<u32>,
}

/// Subset of [`winit::keyboard::NamedKey`] we map explicitly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[expect(dead_code, reason = "Consumed by input systems in Phase 3+")]
pub enum NamedKey {
    Escape,
    F1,
    F3,
    F8,
    F9,
    Space,
}
