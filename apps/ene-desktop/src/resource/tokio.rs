//! Captured from `main` so async subsystems (AI bridge, tray event
//! pump) can `tokio::spawn` from synchronous contexts via
//! [`Handle::current`].
use bevy_ecs::prelude::*;
use tokio::runtime::Handle;

#[derive(Resource, Debug, Clone)]
pub struct TokioHandle(pub Handle);
