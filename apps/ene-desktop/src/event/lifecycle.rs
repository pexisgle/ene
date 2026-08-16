use bevy_ecs::prelude::*;

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
