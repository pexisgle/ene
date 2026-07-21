//! AI plugin.
//!
//! Owns the [`AiBridgeResource`] / [`ProcessingFlag`]
//! resources and the per-action AI consumer systems.
//!
//! Phase 6 does *not* construct the [`AiBridge`] itself;
//! `Runtime::resumed` does that (the bridge needs the
//! tokio handle and the cross-thread event sender). The
//! plugin only owns the bevy-side accessors.
use bevy_app::{App, Plugin, Update};
use bevy_ecs::prelude::*;

use crate::schedule::AppSet;
use crate::system::ui_consumers::{
    apply_ai_permission_system, apply_ai_stream_error_system, apply_ai_stream_finished_system,
    apply_ai_text_deltas_system, apply_ai_user_input_system, apply_cancel_system,
    apply_emote_tokens_system, apply_emotions_system, apply_expression_commands_system,
    apply_motion_commands_system, collect_permission_requests_system, open_chat_system,
    open_settings_system,
};

#[derive(Default)]
pub struct AiPlugin;

impl Plugin for AiPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            (
                open_settings_system,
                open_chat_system,
                apply_ai_text_deltas_system,
                apply_ai_stream_finished_system,
                apply_ai_stream_error_system,
                apply_ai_permission_system,
                collect_permission_requests_system,
                apply_ai_user_input_system,
                apply_emotions_system,
                apply_emote_tokens_system,
                apply_expression_commands_system,
                apply_cancel_system,
                apply_motion_commands_system,
            )
                .in_set(AppSet::Settings),
        );
    }
}
