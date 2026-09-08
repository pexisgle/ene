//! AI stream messages emitted by the event pump.
use bevy_ecs::prelude::*;

use crate::settings::UserInputPrompt;

#[derive(Message, Debug, Clone)]
pub struct AiTextDelta(pub String);

#[derive(Message, Debug, Clone, Copy)]
pub struct AiStreamFinished;

#[derive(Message, Debug, Clone)]
pub struct AiStreamError(pub String);

#[derive(Message, Debug, Clone)]
pub struct AiPermissionRequested {
    pub request_id: String,
    pub action: String,
    pub target: String,
    pub description: String,
}

#[derive(Message, Debug, Clone)]
pub struct AiUserInputRequested {
    pub request_id: String,
    pub prompt: UserInputPrompt,
}

#[derive(Message, Debug, Clone)]
pub struct PerformanceCue(pub String);

pub use PerformanceCue as EmoteToken;

#[derive(Message, Debug, Clone)]
pub struct MotionCommand {
    pub name: String,
    pub layer: ene_card::MotionLayer,
    pub priority: u8,
    pub duration: f32,
}

#[derive(Message, Debug, Clone)]
pub struct ExpressionCommand {
    pub name: String,
    pub weight: f32,
    pub hold_secs: f64,
    pub target_time: f64,
}

#[derive(Message, Debug, Clone)]
pub struct CancelCommand(pub String);

#[derive(Message, Debug, Clone, Copy)]
pub struct BeatPulse {
    pub bpm: f32,
    pub intensity: f32,
}

#[derive(Message, Debug, Clone, Copy)]
pub struct PendingCandidatesCount(pub usize);
