//! AI stream messages emitted by the legacy bus pump.
//!
//! These mirror the existing [`crate::events::AiStreamUpdate`] variants
//! in a flat, bevy-friendly form. They are produced by the
//! `pump_legacy_events` system in the `First` stage and consumed by
//! the AI consumer systems in the `Update` stage.
use bevy_ecs::prelude::*;
use ene_core::RequestId;
use ene_tool_proto::UserInputPrompt;

#[derive(Message, Debug, Clone)]
#[expect(dead_code, reason = "Consumed by Phase 2 consumer systems")]
pub struct AiTextDelta(pub String);

#[derive(Message, Debug, Clone, Copy)]
pub struct AiStreamFinished;

#[derive(Message, Debug, Clone)]
#[expect(dead_code, reason = "Consumed by Phase 2 consumer systems")]
pub struct AiPermissionRequested {
    pub request_id: RequestId,
    pub action: String,
    pub target: String,
    pub description: String,
}

#[derive(Message, Debug, Clone)]
#[expect(dead_code, reason = "Consumed by Phase 2 consumer systems")]
pub struct AiUserInputRequested {
    pub request_id: RequestId,
    pub prompt: UserInputPrompt,
}

#[derive(Message, Debug, Clone)]
#[expect(dead_code, reason = "Consumed by Phase 2 consumer systems")]
pub struct EmoteToken(pub String);
