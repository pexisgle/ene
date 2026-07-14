//! `OpenSettings` / AI consumer systems.
//!
//! Chat-related consumers target the [`ChatWindow`] entity (#109).
//! Settings visibility is handled separately via [`UiWindow`].
use bevy_ecs::prelude::*;

use crate::character_state::EmotionCommand;
use crate::component::chat::{ChatStateComponent, ChatWindow};
use crate::component::ui::{UiStateComponent, UiWindow};
use crate::event::ai::{
    AiPermissionRequested, AiStreamError, AiStreamFinished, AiTextDelta, AiUserInputRequested,
    CancelCommand, EmoteToken, ExpressionCommand, LookAtTarget, MotionCommand,
};
use crate::event::chat::OpenChat;
use crate::event::settings::OpenSettings;
use crate::resource::emotion_pipeline::EmotionPipelineState;
use crate::resource::motion_layer::MotionLayerState;
use crate::settings::QuestionDraft;

pub fn open_settings_system(
    mut events: MessageReader<OpenSettings>,
    mut ui_query: Query<&mut UiStateComponent, With<UiWindow>>,
) {
    let Some(last) = events.read().last() else {
        return;
    };
    let Some(mut ui) = ui_query.iter_mut().next() else {
        return;
    };
    ui.0.settings_window_visible = true;
    if let Some(page) = last.page {
        ui.0.focused_page = Some(page);
    }
}

pub fn open_chat_system(
    mut events: MessageReader<OpenChat>,
    mut chat_query: Query<&mut ChatStateComponent, With<ChatWindow>>,
) {
    if events.read().last().is_none() {
        return;
    }
    let Some(mut chat) = chat_query.iter_mut().next() else {
        return;
    };
    chat.0.chat_window_visible = true;
}

pub fn apply_ai_text_deltas_system(
    mut events: MessageReader<AiTextDelta>,
    mut chat_query: Query<&mut ChatStateComponent, With<ChatWindow>>,
) {
    let Some(mut chat) = chat_query.iter_mut().next() else {
        return;
    };
    for delta in events.read() {
        chat.0.append_text_delta(&delta.0);
    }
}

pub fn apply_ai_stream_finished_system(
    mut events: MessageReader<AiStreamFinished>,
    mut chat_query: Query<&mut ChatStateComponent, With<ChatWindow>>,
) {
    if events.read().last().is_none() {
        return;
    }
    let Some(mut chat) = chat_query.iter_mut().next() else {
        return;
    };
    chat.0.finish_streaming();
}

pub fn apply_ai_stream_error_system(
    mut events: MessageReader<AiStreamError>,
    mut chat_query: Query<&mut ChatStateComponent, With<ChatWindow>>,
) {
    let Some(last) = events.read().last() else {
        return;
    };
    let Some(mut chat) = chat_query.iter_mut().next() else {
        return;
    };
    chat.0.finish_streaming_with_error(&last.0);
}

pub fn apply_ai_permission_system(
    mut events: MessageReader<AiPermissionRequested>,
    mut chat_query: Query<&mut ChatStateComponent, With<ChatWindow>>,
) {
    let Some(last) = events.read().last() else {
        return;
    };
    let Some(mut chat) = chat_query.iter_mut().next() else {
        return;
    };
    chat.0.pending_permission = Some(crate::settings::PendingPermission {
        request_id: last.request_id.clone(),
        action: last.action.clone(),
        target: last.target.clone(),
        description: if last.description.is_empty() {
            None
        } else {
            Some(last.description.clone())
        },
    });
    chat.0.chat_window_visible = true;
}

pub fn apply_ai_user_input_system(
    mut events: MessageReader<AiUserInputRequested>,
    mut chat_query: Query<&mut ChatStateComponent, With<ChatWindow>>,
) {
    let Some(last) = events.read().last() else {
        return;
    };
    let Some(mut chat) = chat_query.iter_mut().next() else {
        return;
    };
    chat.0.pending_user_input = Some(crate::settings::PendingUserInput {
        request_id: last.request_id.clone(),
        prompt: last.prompt.clone(),
    });
    chat.0.user_input_drafts = last
        .prompt
        .items
        .iter()
        .map(|_| QuestionDraft::default())
        .collect();
    chat.0.chat_window_visible = true;
}

pub fn apply_emotions_system(
    mut ui_query: Query<&mut crate::component::ui::UiEmotionQueue, With<UiWindow>>,
    mut pipeline: ResMut<EmotionPipelineState>,
) {
    let Some(mut queue) = ui_query.iter_mut().next() else {
        return;
    };
    while let Some(cmd) = queue.0.commands.pop_front() {
        pipeline.pending.push_back(cmd);
    }
}

pub fn apply_emote_tokens_system(
    mut events: MessageReader<EmoteToken>,
    mut pipeline: ResMut<EmotionPipelineState>,
) {
    let hold_secs = pipeline.expression_hold_secs;
    for token in events.read() {
        pipeline.pending.push_back(EmotionCommand {
            emotion: token.0.clone(),
            target_time: 0.0,
            hold_secs,
            weight: 1.0,
        });
    }
}

/// Feeds [`ExpressionCommand`] messages into the [`EmotionPipelineState`] (#132).
pub fn apply_expression_commands_system(
    mut events: MessageReader<ExpressionCommand>,
    mut pipeline: ResMut<EmotionPipelineState>,
) {
    for cmd in events.read() {
        pipeline.pending.push_back(EmotionCommand {
            emotion: cmd.name.clone(),
            target_time: 0.0,
            hold_secs: cmd.hold_secs,
            weight: cmd.weight,
        });
    }
}

/// Feeds [`LookAtTarget`] messages into the motion layer (#132).
///
/// Look-at is handled by the existing per-frame look-at evaluator
/// in `Runtime::render_char_frame`. This system is a placeholder
/// for future explicit look-at cue routing.
pub fn apply_lookat_system(mut events: MessageReader<LookAtTarget>) {
    for _cmd in events.read() {
        // LookAtTarget(target, priority) — routed per-frame by
        // Runtime::update_look_at when cursor tracking is active.
    }
}

/// Applies [`CancelCommand`] to clear expression or motion state (#132).
pub fn apply_cancel_system(
    mut events: MessageReader<CancelCommand>,
    mut pipeline: ResMut<EmotionPipelineState>,
    mut state: ResMut<MotionLayerState>,
) {
    for cmd in events.read() {
        match cmd.0.as_str() {
            "expr" | "expression" => {
                pipeline.pending.clear();
                pipeline.active = None;
            }
            "motion" => {
                state.0.cancel_all_motions();
            }
            "all" => {
                pipeline.pending.clear();
                pipeline.active = None;
                state.0.cancel_all_motions();
            }
            _ => {}
        }
    }
}

/// Feeds [`MotionCommand`] messages into the [`MotionLayerState`] (#133).
pub fn apply_motion_commands_system(
    mut events: MessageReader<MotionCommand>,
    mut state: ResMut<MotionLayerState>,
) {
    for cmd in events.read() {
        let layer = match cmd.layer.as_str() {
            "upper" => ene_vrm::layer_composer::MotionLayer::Upper,
            "lower" => ene_vrm::layer_composer::MotionLayer::Lower,
            _ => ene_vrm::layer_composer::MotionLayer::Full,
        };
        state.0.accept_motion(cmd.name.clone(), layer, cmd.priority);
    }
}

#[cfg(test)]
#[path = "ui_consumers_tests.rs"]
mod tests;
