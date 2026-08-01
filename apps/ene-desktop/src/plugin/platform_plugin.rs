//! Platform plugin.
//!
//! Owns the per-frame Linux display-server integration
//! systems plus the cross-platform `should_render_debug`
//! gate. The per-handle Linux display-server state lives in
//! bevy `Resource`s; this plugin wires up the systems that
//! consume them.
//!
//! On non-Linux builds the Linux-only resources + systems
//! are skipped; the cross-platform
//! `should_render_debug_system` + `apply_linux_click_through_system`
//! (no-op on non-Linux) still run.
use bevy_app::{App, Plugin, Update};
use bevy_ecs::schedule::IntoScheduleConfigs;

use crate::resource::cursor_state::CursorState;
#[cfg(target_os = "linux")]
use crate::resource::platform_state::resources::{
    LastAppliedInputRects, LastInputSource, LayerShell, LayerShellFreeze, MaskCapture,
    MaskReadbackWorkerRes, WaylandInputRegion, X11ContextRes,
};
#[cfg(target_os = "linux")]
use crate::resource::tray::GtkReady;
use crate::schedule::AppSet;
use crate::system::platform::click_through::apply_linux_click_through_system;
use crate::system::platform::cursor::update_cursor_state_system;
#[cfg(target_os = "linux")]
use crate::system::platform::gtk_pump::tick_gtk_system;
use crate::system::platform::should_render_debug::{
    DebugFps, DragActive, LastDebugUpdate, ShouldRenderDebug, TransparentWindow,
    should_render_debug_system,
};

#[derive(Default)]
pub struct PlatformPlugin;

impl Plugin for PlatformPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(CursorState::default());

        // Cross-platform per-frame resources read by the
        // `should_render_debug_system` + the Linux
        // `apply_linux_click_through_system`. The winit event
        // handler writes them on every relevant input.
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
            // Initialised to `false` so the GTK pump systems in
            // `Update` / `Last` early-return until `Runtime::resumed`
            // has flipped it to `true` after `gtk::init` succeeds.
            // The first `App::update` runs inside `Runtime::new`
            // — before winit dispatches `resumed` — and would
            // otherwise panic in `gtk::events_pending`.
            app.init_resource::<GtkReady>();
        }

        app.add_systems(
            Update,
            (
                should_render_debug_system.in_set(AppSet::Input),
                update_cursor_state_system.in_set(AppSet::Input),
                apply_linux_click_through_system.in_set(AppSet::Settings),
            )
                .chain(),
        );

        #[cfg(target_os = "linux")]
        app.add_systems(Update, tick_gtk_system.in_set(AppSet::Settings));
    }
}
