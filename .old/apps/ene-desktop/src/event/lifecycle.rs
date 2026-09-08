use bevy_ecs::prelude::*;

/// User clicked the window's close button. The runtime exits on
/// this event.
#[derive(Message, Debug, Clone, Copy)]
pub struct WindowCloseRequested;

/// The runtime actor broadcast channel closed.
#[derive(Message, Debug, Clone, Copy)]
pub struct RuntimeDisconnected;
