//! Window lifecycle messages.
use bevy_ecs::prelude::*;

/// Window was resized or its DPI scale changed. The render system
/// reconfigures the swapchain on this event.
#[derive(Message, Debug, Clone, Copy)]
#[expect(dead_code, reason = "Consumed by the render system in Phase 7")]
pub struct WindowResized {
    pub width: u32,
    pub height: u32,
}

/// User clicked the window's close button. The runtime exits on
/// this event.
#[derive(Message, Debug, Clone, Copy)]
pub struct WindowCloseRequested;
