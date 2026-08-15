//! Written by `AppState::with_channel` when `AiBridge::try_new` fails
//! and read/consumed in `Runtime::new` to surface a fatal dialog.
//! A bevy `Resource` bridges the gap between `with_channel` (where the
//! UI entity does not yet exist) and `Runtime::new` (where it does).

/// Startup error payload carried from `with_channel` to the first
/// winit callback. `Some(msg)` when the runtime actor failed to
/// open; removed (via `take` / `remove`) once consumed.
#[derive(bevy_ecs::prelude::Resource)]
pub struct RuntimeStartupError(pub Option<String>);
