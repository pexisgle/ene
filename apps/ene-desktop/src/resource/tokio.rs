//! Tokio runtime handle.
//!
//! Captured from `main` so async subsystems (AI bridge, tray event
//! pump) can `tokio::spawn` from synchronous contexts via
//! [`Handle::current`].
use bevy_ecs::prelude::*;
use tokio::runtime::Handle;

/// Tokio multi-thread runtime handle.
#[derive(Resource, Debug, Clone)]
#[expect(dead_code, reason = "Consumed by AI plugin in Phase 6")]
pub struct TokioHandle(pub Handle);
