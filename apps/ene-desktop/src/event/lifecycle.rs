//! Window lifecycle messages.
use bevy_ecs::prelude::*;

/// Window was resized or its DPI scale changed. The render system
/// reconfigures the swapchain on this event.
#[derive(Message, Debug, Clone, Copy)]
#[expect(dead_code, reason = "yet to be wired to render system")]
pub struct WindowResized {
    pub width: u32,
    pub height: u32,
}

/// User clicked the window's close button. The runtime exits on
/// this event.
#[derive(Message, Debug, Clone, Copy)]
pub struct WindowCloseRequested;

/// Written once per frame from the tray subsystem when the GTK
/// main loop integration has pending events. The `tick_gtk_system`
/// (in `PlatformPlugin`) drains the message and pumps the GTK queue.
#[derive(Message, Debug, Clone, Copy)]
pub struct TickGtk;

/// The runtime actor broadcast channel closed.
#[derive(Message, Debug, Clone, Copy)]
pub struct RuntimeDisconnected;
