//! Linux-only: forward the masked silhouette rectangles from
//! the [`MaskCapture`] worker into the [`LastAppliedInputRects`]
//! / [`LastInputSource`] bevy `Resource`s.
//!
//! Reads the cached `MaskCapture` snapshot. If a `readback` is
//! fresh the rectangles are scaled to surface-local coordinates
//! and published into the `LastApplied*` resources; the
//! `apply_linux_click_through_system` then either applies them
//! to the Wayland / X11 contexts or overrides the policy based
//! on the cursor / drag state.
//!
//! On non-Linux builds the module is a no-op.
#[cfg(target_os = "linux")]
mod imp {
    use bevy_ecs::prelude::*;

    use crate::input_region_debug::InputRegionSource;
    use crate::platform::wayland_region::Rect;
    use crate::resource::platform_state::resources::{
        LastAppliedInputRects, LastInputSource, MaskCapture,
    };

    /// Cheap to run every frame (no GPU work, just a `lock` + walk).
    pub fn refresh_input_region_system(
        mask: Option<Res<MaskCapture>>,
        mut last_rects: ResMut<LastAppliedInputRects>,
        mut last_source: ResMut<LastInputSource>,
    ) {
        let Some(mask_res) = mask else {
            last_rects.0.clear();
            last_source.0 = InputRegionSource::Empty;
            return;
        };
        let Some(cam) = mask_res.0.as_ref() else {
            last_rects.0.clear();
            last_source.0 = InputRegionSource::Empty;
            return;
        };
        let guard = cam.lock();
        let extracted = guard.extract_rectangles();
        if extracted.is_empty() {
            last_rects.0.clear();
            last_source.0 = InputRegionSource::Empty;
            return;
        }
        let factor = guard.downsample() as i64;
        let scaled: Vec<Rect> = extracted
            .into_iter()
            .map(|(x, y, w, h)| Rect {
                x: (i64::from(x) * factor).min(i64::from(i32::MAX)) as i32,
                y: (i64::from(y) * factor).min(i64::from(i32::MAX)) as i32,
                w: (i64::from(w) * factor).min(i64::from(i32::MAX)) as i32,
                h: (i64::from(h) * factor).min(i64::from(i32::MAX)) as i32,
            })
            .collect();
        last_rects.0 = scaled;
        last_source.0 = InputRegionSource::Mask;
    }

    #[cfg(test)]
    mod tests {
        use bevy_ecs::schedule::Schedule;

        use super::*;
        use crate::platform::wayland_region::Rect;

        fn step_world(world: &mut World) {
            let mut schedule = Schedule::default();
            schedule.add_systems(refresh_input_region_system);
            schedule.run(world);
        }

        fn empty_world() -> World {
            let mut world = World::new();
            world.init_resource::<MaskCapture>();
            world.init_resource::<LastAppliedInputRects>();
            world.init_resource::<LastInputSource>();
            world
        }

        #[test]
        fn no_mask_capture_clears_last_applied() {
            let mut world = empty_world();
            world.resource_mut::<LastAppliedInputRects>().0 = vec![Rect::new(0, 0, 1, 1)];
            world.resource_mut::<LastInputSource>().0 = InputRegionSource::Mask;
            step_world(&mut world);
            assert!(world.resource::<LastAppliedInputRects>().0.is_empty());
            assert_eq!(
                world.resource::<LastInputSource>().0,
                InputRegionSource::Empty
            );
        }

        #[test]
        fn empty_mask_capture_keeps_empty() {
            let mut world = empty_world();
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
pub use imp::refresh_input_region_system;

#[cfg(not(target_os = "linux"))]
pub fn refresh_input_region_system() {}
