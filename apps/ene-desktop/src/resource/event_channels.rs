//! Holds the `tokio::sync::mpsc` halves used to communicate between
//! async subsystems (AI bridge, tray event pump) and the winit event
//! loop; `pump_legacy_events` drains the `rx`.
use bevy_ecs::prelude::*;

use crate::events::{AppEventReceiver, AppEventSender};

#[derive(Resource)]
pub struct EventChannels {
    /// Sender half. Cloned by the AI bridge, tray, and the voice mic
    /// toggle. Without the `voice` feature no production code reads it
    /// (tests still do), so the dead-code expectation is gated on that.
    #[cfg_attr(
        all(not(test), not(feature = "voice")),
        expect(
            dead_code,
            reason = "Used by the voice mic toggle and Phase 3+ systems"
        )
    )]
    pub tx: AppEventSender,
    /// Drained by `pump_legacy_events`.
    pub rx: AppEventReceiver,
}
