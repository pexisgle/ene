//! Per-frame cursor state write.
//!
//! Phase 8 made the cursor source single-source-of-truth: the
//! `device_query` global mouse position is read in
//! `update_char_window_cursor_and_hittest` (in `runtime.rs`)
//! and converted to window-local coordinates. The
//! `update_cursor_state_system` is intentionally a no-op kept
//! as a documented seam for any future migration that wants to
//! route cursor state through bevy.
//!
//! `CursorState` is still a bevy `Resource` and is populated by
//! the F3 collider overlay's per-frame `update_char_window_cursor_and_hittest`
//! indirectly (via `apply_linux_click_through_system` reading
//! the resource on Linux).
use bevy_ecs::prelude::*;

use crate::event::input::PointerMoved;
use crate::resource::cursor_state::CursorState;

/// Intentionally a no-op.
///
/// Phase 8 picks `device_query` (a global screen coordinate) as
/// the cursor source of truth for the click-through test because
/// winit 0.30's `WindowEvent::CursorMoved` on Wayland is not
/// reliable on all compositors. This system keeps its signature
/// and slot in `AppSet::Input` so the migration is local: a
/// future Phase can re-introduce the `PointerMoved` reader
/// without changing the plugin wiring.
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "Slot reserved for the future PointerMoved-based cursor source; Phase 8 uses device_query"
    )
)]
pub fn update_cursor_state_system(
    _events: MessageReader<PointerMoved>,
    _cursor: ResMut<CursorState>,
) {
}

#[cfg(test)]
mod tests {
    use bevy_ecs::message::Messages;
    use bevy_ecs::schedule::Schedule;

    use super::*;

    fn step_world(world: &mut World) {
        let mut schedule = Schedule::default();
        schedule.add_systems(update_cursor_state_system);
        schedule.run(world);
    }

    #[test]
    fn no_events_leaves_cursor_at_default() {
        let mut world = World::new();
        world.init_resource::<Messages<PointerMoved>>();
        world.init_resource::<CursorState>();
        step_world(&mut world);
        assert!(world.resource::<CursorState>().physical.is_none());
    }

    #[test]
    fn pointer_moved_event_does_not_overwrite_cursor() {
        let mut world = World::new();
        world.init_resource::<Messages<PointerMoved>>();
        world.init_resource::<CursorState>();
        world.write_message(PointerMoved {
            logical_x: 12.5,
            logical_y: -7.0,
        });
        step_world(&mut world);
        // Phase 8: device_query wins. PointerMoved must NOT
        // overwrite the cursor state.
        assert!(world.resource::<CursorState>().physical.is_none());
    }
}
