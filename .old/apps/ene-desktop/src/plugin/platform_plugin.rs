//! Platform plugin.
//!
//! Owns the per-frame Linux display-server integration systems plus the
//! cross-platform `should_render_debug` gate.
use bevy_app::{App, Plugin, Update};
use bevy_ecs::schedule::IntoScheduleConfigs;

use crate::resource::cursor_state::CursorState;
#[cfg(target_os = "linux")]
use crate::resource::platform_state::resources::{
    LastAppliedInputRects, LastInputSource, LayerShell, LayerShellFreeze, MaskCapture,
    MaskReadbackWorkerRes, WaylandInputRegion, X11ContextRes,
};
use crate::schedule::AppSet;
use crate::system::platform::click_through::apply_linux_click_through_system;
use crate::system::platform::should_render_debug::{
    DebugFps, DragActive, LastDebugUpdate, ShouldRenderDebug, TransparentWindow,
    should_render_debug_system,
};

#[derive(Default)]
pub struct PlatformPlugin;

impl Plugin for PlatformPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(CursorState::default());

        app.init_resource::<DragActive>();
        app.init_resource::<DebugFps>();
        app.init_resource::<LastDebugUpdate>();
        app.init_resource::<ShouldRenderDebug>();
        app.init_resource::<TransparentWindow>();

        #[cfg(target_os = "linux")]
        {
            app.insert_resource(WaylandInputRegion::default());
            app.insert_resource(X11ContextRes::default());
            app.insert_resource(LayerShell::default());
            app.insert_resource(LayerShellFreeze::default());
            app.insert_resource(MaskCapture::default());
            app.insert_resource(MaskReadbackWorkerRes::default());
            app.insert_resource(LastAppliedInputRects::default());
            app.insert_resource(LastInputSource::default());
        }

        app.add_systems(
            Update,
            (
                should_render_debug_system.in_set(AppSet::Input),
                apply_linux_click_through_system.in_set(AppSet::Settings),
            )
                .chain(),
        );
    }
}
