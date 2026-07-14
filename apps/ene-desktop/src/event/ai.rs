//! AI stream messages emitted by the legacy bus pump.
//!
//! These mirror the existing [`crate::events::AiStreamUpdate`] variants
//! in a flat, bevy-friendly form. They are produced by the
//! `pump_legacy_events` system in the `First` stage and consumed by
//! the AI consumer systems in the `Update` stage.
use bevy_ecs::prelude::*;
use ene_runtime::RequestId;
use ene_tool_proto::UserInputPrompt;

#[derive(Message, Debug, Clone)]
pub struct AiTextDelta(pub String);

#[derive(Message, Debug, Clone, Copy)]
pub struct AiStreamFinished;

/// Terminal failure or host Busy — message text for the chat UI.
#[derive(Message, Debug, Clone)]
pub struct AiStreamError(pub String);

#[derive(Message, Debug, Clone)]
pub struct AiPermissionRequested {
    pub request_id: RequestId,
    pub action: String,
    pub target: String,
    pub description: String,
}

#[derive(Message, Debug, Clone)]
pub struct AiUserInputRequested {
    pub request_id: RequestId,
    pub prompt: UserInputPrompt,
}

/// Performance cue name mapped toward VRM expression playback by the
/// desktop emotion pipeline (from [`ene_runtime::EneEvent::Performance`]).
/// Does not pass SpecialToken/Expression into the VRM API.
#[derive(Message, Debug, Clone)]
pub struct PerformanceCue(pub String);

/// Compatibility alias for older call sites (same type).
pub use PerformanceCue as EmoteToken;

/// Motion cue routed from [`ene_runtime::EneEvent::Performance`] when
/// the cue kind is [`PerfKind::Motion`] (#133).
#[derive(Message, Debug, Clone)]
pub struct MotionCommand {
    pub name: String,
    pub layer: String,
    pub priority: u8,
}
