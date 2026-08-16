//! Mirrors the Windows path's `Window::set_cursor_hittest` call but routes
//! through the per-display-server `apply_linux_click_through` function so
//! bevy systems own the policy decision.
//!
//! On non-Linux builds the system is a no-op.
#[cfg(target_os = "linux")]
mod imp {
    use bevy_ecs::prelude::*;

    use crate::platform::{ClickThroughInputs, apply_linux_click_through};
    use crate::resource::cursor_state::CursorState;
    use crate::resource::platform_state::resources::{
        LastAppliedInputRects, LastInputSource, LayerShell, LayerShellFreeze, MaskCapture,
        WaylandInputRegion, WindowScaleFactor, X11ContextRes,
    };
    use crate::system::platform::should_render_debug::{
        DragActive, ShouldRenderDebug, TransparentWindow,
    };

    /// Skipped when [`ShouldRenderDebug`] is `false` (FPS
    /// throttle) — the per-frame input region only needs to be
    /// refreshed as often as the debug overlay.
    pub fn apply_linux_click_through_system(
        should_update: Res<ShouldRenderDebug>,
        drag: Res<DragActive>,
        cursor: Res<CursorState>,
        transparent: Res<TransparentWindow>,
        wayland: Res<WaylandInputRegion>,
        x11: Res<X11ContextRes>,
        layer_shell: Res<LayerShell>,
        freeze: Res<LayerShellFreeze>,
        mask: Option<Res<MaskCapture>>,
        scale_factor: Option<Res<WindowScaleFactor>>,
        mut last_rects: ResMut<LastAppliedInputRects>,
        mut last_source: ResMut<LastInputSource>,
    ) {
        if !should_update.0 {
            return;
        }
        let cursor_over = cursor.physical.is_some();
        let drag_is_dragging = drag.0;
        let allows_input = !transparent.0 || cursor_over || drag_is_dragging;
        let (rects, source) = apply_linux_click_through(
            ClickThroughInputs {
                wayland: wayland.0.as_ref(),
                x11: x11.0.as_ref(),
                layer_shell: layer_shell.0.as_ref(),
                layer_shell_freeze: freeze.0,
                mask_capture: mask.as_ref().and_then(|m| m.0.as_ref()),
                drag_is_dragging,
                scale_factor: scale_factor.map_or(1.0, |s| s.0),
            },
            allows_input,
            cursor_over,
            freeze.0,
        );
        last_rects.0 = rects;
        last_source.0 = source;
    }

    #[cfg(test)]
    mod tests {
        use bevy_ecs::schedule::Schedule;

        use super::*;
        use crate::input_region_debug::InputRegionSource;

        fn step_world(world: &mut World) {
            let mut schedule = Schedule::default();
            schedule.add_systems(apply_linux_click_through_system);
            schedule.run(world);
        }

        fn empty_world() -> World {
            let mut world = World::new();
            world.init_resource::<CursorState>();
            world.init_resource::<DragActive>();
            world.init_resource::<TransparentWindow>();
            world.init_resource::<WaylandInputRegion>();
            world.init_resource::<X11ContextRes>();
            world.init_resource::<LayerShell>();
            world.init_resource::<LayerShellFreeze>();
            world.init_resource::<MaskCapture>();
            world.init_resource::<WindowScaleFactor>();
            world.init_resource::<LastAppliedInputRects>();
            world.init_resource::<LastInputSource>();
            world.init_resource::<ShouldRenderDebug>();
            world
        }

        #[test]
        fn transparent_with_no_signal_blocks_input() {
            let mut world = empty_world();
            world.insert_resource(TransparentWindow(true));
            world.insert_resource(ShouldRenderDebug(true));
            step_world(&mut world);
            assert!(world.resource::<LastAppliedInputRects>().0.is_empty());
            assert_eq!(
                world.resource::<LastInputSource>().0,
                InputRegionSource::Empty
            );
        }

        #[test]
        fn drag_passes_input_even_when_transparent() {
            let mut world = empty_world();
            world.insert_resource(TransparentWindow(true));
            world.insert_resource(DragActive(true));
            world.insert_resource(ShouldRenderDebug(true));
            step_world(&mut world);
            assert!(!world.resource::<LastAppliedInputRects>().0.is_empty());
        }

        #[test]
        fn freeze_passes_input_even_when_transparent() {
            let mut world = empty_world();
            world.insert_resource(TransparentWindow(true));
            world.insert_resource(LayerShellFreeze(true));
            world.insert_resource(ShouldRenderDebug(true));
            step_world(&mut world);
            assert!(!world.resource::<LastAppliedInputRects>().0.is_empty());
        }

        #[test]
        fn throttled_skips_dispatch() {
            let mut world = empty_world();
            world.insert_resource(TransparentWindow(false));
            world.insert_resource(ShouldRenderDebug(false));
            step_world(&mut world);
            assert!(world.resource::<LastAppliedInputRects>().0.is_empty());
            assert_eq!(
                world.resource::<LastInputSource>().0,
                InputRegionSource::Empty
            );
        }

        #[test]
        fn opaque_window_uses_mask_capture_when_no_cursor() {
            // `transparent=false` makes `allows_input=true` but
            // the platform function only emits a full-window rect
            // when the cursor is over the silhouette or the user
            // is dragging. With no mask capture the rects are
            // empty (opaque + idle + no mask = empty).
            let mut world = empty_world();
            world.insert_resource(TransparentWindow(false));
            world.insert_resource(ShouldRenderDebug(true));
            step_world(&mut world);
            assert!(world.resource::<LastAppliedInputRects>().0.is_empty());
            assert_eq!(
                world.resource::<LastInputSource>().0,
                InputRegionSource::Empty
            );
        }
    }
}

#[cfg(target_os = "linux")]
pub use imp::apply_linux_click_through_system;

#[cfg(not(target_os = "linux"))]
pub fn apply_linux_click_through_system() {}
