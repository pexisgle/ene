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
    CancelCommand, EmoteToken, ExpressionCommand, MotionCommand, PendingCandidatesCount,
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

pub fn apply_runtime_disconnected_system(
    mut events: MessageReader<crate::event::lifecycle::RuntimeDisconnected>,
    mut ui_query: Query<&mut UiStateComponent, With<UiWindow>>,
) {
    if events.read().last().is_none() {
        return;
    }
    let Some(mut ui) = ui_query.iter_mut().next() else {
        return;
    };
    ui.0.runtime_disconnected = true;
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

/// Accumulates pending tool-permission approvals into the settings
/// `UiState` so the Permission Center page can list and answer them
/// (#177). Runs alongside [`apply_ai_permission_system`], which keeps
/// the chat-window dialog; the two are independent surfaces over the
/// same `decide_permission` call. Duplicate `request_id`s are ignored
/// so a re-broadcast never double-lists a request.
pub fn collect_permission_requests_system(
    mut events: MessageReader<AiPermissionRequested>,
    mut ui_query: Query<&mut UiStateComponent, With<UiWindow>>,
) {
    let Some(mut ui) = ui_query.iter_mut().next() else {
        return;
    };
    for event in events.read() {
        let already_present =
            ui.0.permission_requests
                .iter()
                .any(|p| p.request_id == event.request_id);
        if already_present {
            continue;
        }
        ui.0.permission_requests
            .push(crate::settings::PendingPermission {
                request_id: event.request_id.clone(),
                action: event.action.clone(),
                target: event.target.clone(),
                description: if event.description.is_empty() {
                    None
                } else {
                    Some(event.description.clone())
                },
            });
    }
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

/// Converts the canonical [`ene_config::MotionLayer`] (carried on performance
/// cues and cancel scopes) into the rendering-side [`ene_vrm::MotionLayer`].
///
/// `ene-vrm` is rendering-only and cannot depend on `ene-config`, and the
/// orphan rule blocks a cross-crate `From` impl, so the desktop bridges the
/// two mirrors here by matching variants directly (#133).
fn to_vrm_layer(layer: ene_config::MotionLayer) -> ene_vrm::MotionLayer {
    match layer {
        ene_config::MotionLayer::Upper => ene_vrm::MotionLayer::Upper,
        ene_config::MotionLayer::Lower => ene_vrm::MotionLayer::Lower,
        ene_config::MotionLayer::Full => ene_vrm::MotionLayer::Full,
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
                state.cancel_all_motions();
            }
            "all" => {
                pipeline.pending.clear();
                pipeline.active = None;
                state.cancel_all_motions();
            }
            scope if scope.starts_with("motion:") => {
                let label = scope.strip_prefix("motion:").unwrap_or_default();
                // Unknown layer labels fall back to `Full` (preempts
                // everything), matching the pre-typing behavior.
                let layer = ene_config::MotionLayer::from_label(label)
                    .unwrap_or(ene_config::MotionLayer::Full);
                state.cancel_motion(to_vrm_layer(layer));
            }
            _ => {
                tracing::debug!(
                    component = "ApplyCancel",
                    scope = %cmd.0,
                    "Unknown cancel scope; ignoring"
                );
            }
        }
    }
}

/// Feeds [`MotionCommand`] messages into the [`MotionLayerState`] (#133).
///
/// Converts the canonical [`ene_config::MotionLayer`] carried on the command
/// into the rendering-side [`ene_vrm::MotionLayer`] via [`to_vrm_layer`] — no
/// string round-trip.
pub fn apply_motion_commands_system(
    mut events: MessageReader<MotionCommand>,
    mut state: ResMut<MotionLayerState>,
) {
    for cmd in events.read() {
        let layer = to_vrm_layer(cmd.layer);
        let repeat = match layer {
            ene_vrm::MotionLayer::Lower => ene_vrm::RepeatMode::Loop,
            _ => ene_vrm::RepeatMode::Once,
        };
        state.accept_motion(cmd.name.clone(), layer, cmd.priority, cmd.duration, repeat);
    }
}

/// Updates the pending-candidates badge in `UiState` when new candidates
/// are available (#223).
pub fn apply_pending_candidates_count_system(
    mut events: MessageReader<PendingCandidatesCount>,
    mut ui_query: Query<&mut UiStateComponent, With<UiWindow>>,
) {
    let Some(last) = events.read().last() else {
        return;
    };
    let Some(mut ui) = ui_query.iter_mut().next() else {
        return;
    };
    ui.0.memory_journal_pending_count = last.0;
}

#[cfg(test)]
#[path = "ui_consumers_tests.rs"]
mod tests;
