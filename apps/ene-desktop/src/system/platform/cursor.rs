//! Per-frame cursor state write.
//!
//! Drains the `PointerMoved` message stream and updates the
//! [`CursorState`](crate::resource::cursor_state::CursorState)
//! resource. The `apply_linux_click_through_system` and the
//! raycast hit-test system both read it on subsequent stages.
use bevy_ecs::prelude::*;
use winit::dpi::PhysicalPosition;

use crate::event::input::PointerMoved;
use crate::resource::cursor_state::CursorState;

pub fn update_cursor_state_system(
    mut events: MessageReader<PointerMoved>,
    mut cursor: ResMut<CursorState>,
) {
    let Some(last) = events.read().last() else {
        return;
    };
    cursor.physical = Some(PhysicalPosition::new(
        f64::from(last.logical_x),
        f64::from(last.logical_y),
    ));
}
