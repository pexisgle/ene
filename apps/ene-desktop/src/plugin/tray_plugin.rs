//! Tray plugin.
//!
//! Phase 6 placeholder. The tray icon is still constructed
//! by `Runtime::resumed` (it needs a live `winit::Window` to
//! call `WaylandInputRegionContext::try_new`), and the
//! `pump_tray_events` background thread already forwards
//! tray clicks into the `AppEvent::Tray(_)` cross-thread
//! channel. The pump system reads those events via the
//! `pump_legacy_events` system in `First` and writes the
//! typed `OpenSettings { page }` / `SettingsActionEvent::Quit`
//! messages.
//!
//! Phase 7.5: `pump_legacy_events` publishes a `TickGtk`
//! message every frame on Linux; `tick_gtk_system` drains
//! the queue. The actual `tick_gtk()` call still lives in
//! `Runtime::about_to_wait` because the
//! `Rc<RefCell<TrayHandle>>` is not `Send + Sync`.
use bevy_app::{App, Plugin};

#[derive(Default)]
pub struct TrayPlugin;

impl Plugin for TrayPlugin {
    fn build(&self, _app: &mut App) {
        #[cfg(target_os = "linux")]
        {
            use crate::schedule::AppSet;
            use crate::system::tray_tick::tick_gtk_system;
            use bevy_ecs::schedule::IntoScheduleConfigs;
            _app.add_systems(bevy_app::Last, tick_gtk_system.in_set(AppSet::Present));
        }
    }
}
