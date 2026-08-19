//! The tray icon is constructed by `Runtime::resumed` (it needs a
//! live `winit::Window` to call `WaylandInputRegionContext::try_new`);
//! tray clicks reach the `AppEvent::Tray(_)` cross-thread channel and
//! are translated into typed `OpenSettings` / `Quit` messages by
//! `pump_legacy_events`.
//!
//! On Linux, `pump_legacy_events` publishes a `TickGtk` message every
//! frame; `tick_gtk_system` drains the queue. The actual `tick_gtk()`
//! call lives in `Runtime::about_to_wait` because the
//! `Rc<RefCell<TrayHandle>>` is not `Send + Sync`.
use bevy_app::{App, Plugin};

#[derive(Default)]
pub struct TrayPlugin;

impl Plugin for TrayPlugin {
    fn build(
        &self,
        #[cfg(target_os = "linux")] app: &mut App,
        #[cfg(not(target_os = "linux"))] _app: &mut App,
    ) {
        #[cfg(target_os = "linux")]
        {
            use crate::schedule::AppSet;
            use crate::system::tray_tick::tick_gtk_system;
            use bevy_ecs::schedule::IntoScheduleConfigs;
            app.add_systems(bevy_app::Last, tick_gtk_system.in_set(AppSet::Present));
        }
    }
}
