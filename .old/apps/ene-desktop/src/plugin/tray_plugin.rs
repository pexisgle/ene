//! The tray icon is constructed by `Runtime::resumed`; tray clicks reach the
//! `AppEvent::Tray(_)` cross-thread channel and are translated into typed
//! messages by `pump_legacy_events`.
use bevy_app::Plugin;

#[derive(Default)]
pub struct TrayPlugin;

impl Plugin for TrayPlugin {
    fn build(&self, _app: &mut bevy_app::App) {}
}
