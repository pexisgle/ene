use bevy::prelude::*;
use bevy::transform::TransformSystems;

mod capture;
mod region;
mod render_layers;
mod taskbar;

pub(super) fn register(app: &mut App) {
    app.init_resource::<capture::WaylandVisibleRectState>()
        .init_resource::<capture::WaylandMaskCaptureState>()
        .init_resource::<capture::WaylandPolygonLagCompensationState>()
        .init_resource::<capture::WaylandMaskBuffers>()
        .init_resource::<capture::WaylandRectExtractionBuffers>()
        .init_non_send_resource::<taskbar::LinuxTaskbarState>()
        .init_non_send_resource::<region::WaylandInputRegionState>()
        .add_observer(capture::on_wayland_mask_readback_complete)
        .add_systems(Startup, capture::spawn_wayland_mask_capture)
        .add_systems(
            PostUpdate,
            (
                taskbar::sync_primary_window_taskbar_visibility,
                capture::sync_wayland_mask_capture,
                capture::sync_wayland_polygon_lag_compensation,
                render_layers::sync_character_render_layers,
                region::apply_wayland_input_region,
                capture::draw_visible_rect_gizmos,
            )
                .chain()
                .after(TransformSystems::Propagate),
        );
}
