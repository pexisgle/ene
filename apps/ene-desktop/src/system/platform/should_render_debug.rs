//! Per-frame "should the debug overlay update this frame?" gate.
//!
//! The system reads the [`DragActive`] resource (written by the winit
//! event handler on every mouse-down/up) and writes a
//! [`ShouldRenderDebug`] marker that the per-frame cursor hittest can
//! read to short-circuit when the rate limit is in effect.
use std::time::{Duration, Instant};

use bevy_ecs::prelude::*;

/// Latest "is the user currently dragging the character?"
/// signal, exposed as a bevy `Resource` so the per-frame gate
/// can run as a system.
#[derive(Resource, Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct DragActive(pub bool);

/// Per-frame debug FPS, mirroring
/// `settings.graphics.debug_fps`. Written by the winit event
/// handler on every settings change.
#[derive(Resource, Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct DebugFps(pub u32);

/// Whether the character window is in "transparent" mode (the
/// user pressed `Space` to toggle decorations + click-through
/// off). Read by the per-frame click-through dispatcher on
/// Linux.
#[derive(Resource, Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct TransparentWindow(pub bool);

/// Output of [`should_render_debug_system`]: `true` if the
/// per-frame debug update + raycast should run this frame.
#[derive(Resource, Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ShouldRenderDebug(pub bool);

#[derive(Resource, Debug, Default)]
pub struct LastDebugUpdate(Option<Instant>);

/// Decide whether the per-frame debug overlay work should run.
///
/// - When the character is being dragged, always update (the
///   hit-test follows the cursor live).
/// - When `debug_fps == 0`, always update (no throttling).
/// - Otherwise, throttle to `debug_fps` Hz.
pub fn should_render_debug_system(
    drag: Res<DragActive>,
    debug_fps: Res<DebugFps>,
    mut last: ResMut<LastDebugUpdate>,
    mut out: ResMut<ShouldRenderDebug>,
) {
    if drag.0 {
        last.0 = None;
        out.0 = true;
        return;
    }
    let fps = debug_fps.0;
    if fps == 0 {
        out.0 = true;
        return;
    }
    let now = Instant::now();
    let interval = Duration::from_secs_f64(1.0 / f64::from(fps));
    match last.0 {
        Some(prev) if now.duration_since(prev) < interval => {
            out.0 = false;
        }
        _ => {
            last.0 = Some(now);
            out.0 = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy_ecs::schedule::Schedule;

    fn step_world(world: &mut World) {
        let mut schedule = Schedule::default();
        schedule.add_systems(should_render_debug_system);
        schedule.run(world);
    }

    #[test]
    fn dragging_overrides_throttle() {
        let mut world = World::new();
        world.init_resource::<DragActive>();
        world.init_resource::<DebugFps>();
        world.init_resource::<LastDebugUpdate>();
        world.init_resource::<ShouldRenderDebug>();
        world.insert_resource(DragActive(true));
        world.insert_resource(DebugFps(1));
        step_world(&mut world);
        assert!(world.resource::<ShouldRenderDebug>().0);
    }

    #[test]
    fn debug_fps_zero_always_updates() {
        let mut world = World::new();
        world.init_resource::<DragActive>();
        world.init_resource::<DebugFps>();
        world.init_resource::<LastDebugUpdate>();
        world.init_resource::<ShouldRenderDebug>();
        world.insert_resource(DragActive(false));
        world.insert_resource(DebugFps(0));
        step_world(&mut world);
        assert!(world.resource::<ShouldRenderDebug>().0);
    }

    #[test]
    fn throttles_to_debug_fps() {
        let mut world = World::new();
        world.init_resource::<DragActive>();
        world.init_resource::<DebugFps>();
        world.init_resource::<LastDebugUpdate>();
        world.init_resource::<ShouldRenderDebug>();
        world.insert_resource(DragActive(false));
        world.insert_resource(DebugFps(60));
        step_world(&mut world);
        // First frame always runs.
        assert!(world.resource::<ShouldRenderDebug>().0);
        // Second frame within the interval must not.
        step_world(&mut world);
        assert!(!world.resource::<ShouldRenderDebug>().0);
    }

    #[test]
    fn dragging_resets_throttle_clock() {
        let mut world = World::new();
        world.init_resource::<DragActive>();
        world.init_resource::<DebugFps>();
        world.init_resource::<LastDebugUpdate>();
        world.init_resource::<ShouldRenderDebug>();
        world.insert_resource(DragActive(false));
        world.insert_resource(DebugFps(1));
        step_world(&mut world);
        // Second frame is throttled.
        step_world(&mut world);
        assert!(!world.resource::<ShouldRenderDebug>().0);
        // User starts dragging — next frame must run.
        world.insert_resource(DragActive(true));
        step_world(&mut world);
        assert!(world.resource::<ShouldRenderDebug>().0);
        // User stops dragging — clock resets, so the next frame
        // after the drag ends is allowed through again.
        world.insert_resource(DragActive(false));
        step_world(&mut world);
        assert!(world.resource::<ShouldRenderDebug>().0);
    }
}
