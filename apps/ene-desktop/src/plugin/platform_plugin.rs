//! Platform plugin.
//!
//! Owns the per-frame Linux display-server integration
//! systems. Phase 6 lifts the relevant fields out of
//! `state::PlatformState` and into per-handle bevy
//! `Resource`s; this plugin wires up the systems that
//! consume them.
//!
//! On non-Linux builds the plugin is a no-op stub.
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
use crate::system::platform::cursor::update_cursor_state_system;
use crate::system::platform::gtk_pump::tick_gtk_system;
use crate::system::platform::input_region::refresh_input_region_system;

#[derive(Default)]
pub struct PlatformPlugin;

impl Plugin for PlatformPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(CursorState::default());

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
                update_cursor_state_system.in_set(AppSet::Input),
                refresh_input_region_system.in_set(AppSet::Settings),
                apply_linux_click_through_system.in_set(AppSet::Settings),
            )
                .chain(),
        );

        #[cfg(target_os = "linux")]
        app.add_systems(Update, tick_gtk_system.in_set(AppSet::Settings));
    }
}
