//! Event channels resource.
//!
//! Holds the `tokio::sync::mpsc` halves used to communicate between
//! async subsystems (AI bridge, tray event pump) and the winit event
//! loop. During Phase 2 the `pump_legacy_events` system drains the
//! `rx`; in later phases `rx` will be replaced by typed
//! `bevy_ecs::message::Message` channels.
use bevy_ecs::prelude::*;

use crate::events::{AppEventReceiver, AppEventSender};

#[derive(Resource)]
pub struct EventChannels {
    /// Sender half. Cloned by the AI bridge and tray.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "Used by Phase 3+ to push events from systems")
    )]
    pub tx: AppEventSender,
    /// Receiver half. Drained by `pump_legacy_events`.
    pub rx: AppEventReceiver,
}
