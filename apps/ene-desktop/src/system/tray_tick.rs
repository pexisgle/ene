//! Linux-only GTK pump tick.
//!
//! The tray icon library needs the GTK main loop to make progress
//! even when no winit events fire; without a periodic
//! `tick_gtk()` call the system tray stops redrawing.
//!
//! Phase 7.5 introduced the `Messages<TickGtk>` queue and the
//! `pump_legacy_events` system (Linux build of `tray_plugin`)
//! writes one `TickGtk` per `about_to_wait` cycle. The
//! consumer side of the queue is a free function called by
//! `Runtime::about_to_wait` (the winit tick is on the main
//! thread, so the `Rc<RefCell<TrayHandle>>` stays put). The
//! bevy-side `tick_gtk_system` simply drains the queue so the
//! bevy test / introspection world can observe it.
#[cfg(target_os = "linux")]
use bevy_ecs::prelude::*;

#[cfg(target_os = "linux")]
use crate::event::lifecycle::TickGtk;

/// Drains `Messages<TickGtk>`. Linux only. The actual `tick_gtk`
/// call still lives in `Runtime::about_to_wait` because the
/// `Rc<RefCell<TrayHandle>>` is not `Send + Sync`; the system
/// exists so a bevy test can confirm the message is being
/// published every frame.
#[cfg(target_os = "linux")]
pub fn tick_gtk_system(mut messages: MessageReader<TickGtk>) {
    for _ in messages.read() {
        // Drain only; the real call site is in `runtime.rs`.
    }
}
