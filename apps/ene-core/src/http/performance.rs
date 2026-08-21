//! Drain `PerformanceBus` onto the surface WebSocket.

use std::time::Duration;

use ene_body::{LookTarget, MotionLayer, PerformanceCommand, Posture};
use ene_kernel::DisplayDepth;
use ene_session::SoulId;
use serde_json::{Value, json};

use super::AppState;
use super::ws::CoreBus;
use crate::CoreDaemon;

const DRAIN_TICK: Duration = Duration::from_millis(50);

pub async fn run_loop(state: AppState) {
    let mut tick = tokio::time::interval(DRAIN_TICK);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tick.tick().await;
        flush_all(&state.core, &state.events);
    }
}

pub(crate) fn flush_soul(core: &CoreDaemon, events: &CoreBus, soul: SoulId) {
    let Ok(commands) = core.stage().bus().drain(soul) else {
        return;
    };
    for command in commands {
        if let Some(payload) = command_payload(soul, &command) {
            events.emit(DisplayDepth::Surface, payload);
        }
    }
}

fn flush_all(core: &CoreDaemon, events: &CoreBus) {
    for (soul, _) in core.occupants() {
        flush_soul(core, events, soul);
    }
}

fn command_payload(soul: SoulId, command: &PerformanceCommand) -> Option<Value> {
    let soul_id = soul.to_string();
    match command {
        PerformanceCommand::Expression {
            label,
            intensity,
            duration_ms,
        } => Some(json!({
            "type": "body.expression",
            "soul_id": soul_id,
            "name": label,
            "label": label,
            "intensity": intensity,
            "duration_ms": duration_ms,
        })),
        PerformanceCommand::Motion {
            name,
            layer,
            intensity,
        } => Some(json!({
            "type": "body.motion",
            "soul_id": soul_id,
            "name": name,
            "layer": motion_layer_name(*layer),
            "intensity": intensity,
        })),
        PerformanceCommand::LipSync { amplitude, viseme } => Some(json!({
            "type": "body.lipsync",
            "soul_id": soul_id,
            "weight": amplitude,
            "amplitude": amplitude,
            "viseme": viseme.map(ene_body::Viseme::as_str),
            "name": viseme.map(ene_body::Viseme::as_str),
        })),
        PerformanceCommand::LookAt { target, weight } => Some(json!({
            "type": "body.gaze",
            "soul_id": soul_id,
            "target": look_target_name(*target),
            "weight": weight,
        })),
        PerformanceCommand::Posture { pose, blend } => Some(json!({
            "type": "body.posture",
            "soul_id": soul_id,
            "pose": posture_name(*pose),
            "blend": blend,
        })),
    }
}

const fn motion_layer_name(layer: MotionLayer) -> &'static str {
    match layer {
        MotionLayer::Base => "base",
        MotionLayer::Overlay => "overlay",
        MotionLayer::OneShot => "one_shot",
    }
}

const fn look_target_name(target: LookTarget) -> &'static str {
    match target {
        LookTarget::User => "user",
        LookTarget::Away => "away",
        LookTarget::Screen => "screen",
    }
}

const fn posture_name(pose: Posture) -> &'static str {
    match pose {
        Posture::Relax => "relax",
        Posture::Alert => "alert",
        Posture::Thinking => "thinking",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ene_body::Viseme;

    #[test]
    fn expression_payload_carries_soul_and_label() {
        let soul = SoulId::new();
        let payload = command_payload(
            soul,
            &PerformanceCommand::Expression {
                label: "happy".into(),
                intensity: 0.7,
                duration_ms: None,
            },
        )
        .expect("expression is published");
        assert_eq!(payload["type"], "body.expression");
        assert_eq!(payload["soul_id"], soul.to_string());
        assert_eq!(payload["label"], "happy");
        assert_eq!(payload["name"], "happy");
    }

    #[test]
    fn motion_and_lipsync_payloads_use_wire_names() {
        let soul = SoulId::new();
        let motion = command_payload(
            soul,
            &PerformanceCommand::Motion {
                name: "wave".into(),
                layer: MotionLayer::OneShot,
                intensity: Some(1.0),
            },
        )
        .expect("motion is published");
        assert_eq!(motion["type"], "body.motion");
        assert_eq!(motion["name"], "wave");
        assert_eq!(motion["layer"], "one_shot");

        let lips = command_payload(
            soul,
            &PerformanceCommand::LipSync {
                amplitude: 0.4,
                viseme: Some(Viseme::Aa),
            },
        )
        .expect("lipsync is published");
        assert_eq!(lips["type"], "body.lipsync");
        assert_eq!(lips["viseme"], "aa");
    }
}
