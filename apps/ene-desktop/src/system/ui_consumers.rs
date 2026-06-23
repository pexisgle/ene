//! OpenSettings / AI consumer systems.
//!
//! These systems consume the typed `Message`s that the
//! `pump_legacy_events` system in `First` emits and write the
//! resulting per-frame actions into the per-entity
//! [`UiStateComponent`](crate::component::ui::UiStateComponent).
//!
//! The legacy `Runtime::about_to_wait` body still reads
//! `PendingActions` and copies the same data into the same
//! place; Phase 7 deletes the legacy path.
//!
//! All systems run in `AppSet::Settings` so they fire
//! *after* the pump has had a chance to drain the bus and
//! *before* the render path picks up the new state.
use bevy_ecs::prelude::*;

use crate::component::ui::{UiStateComponent, UiWindow};
use crate::event::ai::{AiPermissionRequested, AiTextDelta, AiUserInputRequested};
use crate::event::settings::OpenSettings;
use crate::settings::QuestionDraft;
use crate::settings_ui::PageKind;

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

pub fn apply_ai_text_deltas_system(
    mut events: MessageReader<AiTextDelta>,
    mut ui_query: Query<&mut UiStateComponent, With<UiWindow>>,
) {
    let Some(mut ui) = ui_query.iter_mut().next() else {
        return;
    };
    for delta in events.read() {
        ui.0.ai_latest_response.push_str(&delta.0);
    }
}

pub fn apply_ai_permission_system(
    mut events: MessageReader<AiPermissionRequested>,
    mut ui_query: Query<&mut UiStateComponent, With<UiWindow>>,
) {
    let Some(last) = events.read().last() else {
        return;
    };
    let Some(mut ui) = ui_query.iter_mut().next() else {
        return;
    };
    ui.0.pending_permission = Some(crate::settings::PendingPermission {
        request_id: last.request_id.clone(),
        action: last.action.clone(),
        target: last.target.clone(),
        description: if last.description.is_empty() {
            None
        } else {
            Some(last.description.clone())
        },
    });
    ui.0.focused_page = Some(PageKind::Ai);
    ui.0.settings_window_visible = true;
}

pub fn apply_ai_user_input_system(
    mut events: MessageReader<AiUserInputRequested>,
    mut ui_query: Query<&mut UiStateComponent, With<UiWindow>>,
) {
    let Some(last) = events.read().last() else {
        return;
    };
    let Some(mut ui) = ui_query.iter_mut().next() else {
        return;
    };
    ui.0.pending_user_input = Some(crate::settings::PendingUserInput {
        request_id: last.request_id.clone(),
        prompt: last.prompt.clone(),
    });
    ui.0.user_input_drafts = last
        .prompt
        .items
        .iter()
        .map(|_| QuestionDraft::default())
        .collect();
    ui.0.focused_page = Some(PageKind::Ai);
    ui.0.settings_window_visible = true;
}

/// Phase 7.5: drain the bevy-side [`UiEmotionQueue`](crate::component::ui::UiEmotionQueue)
/// into [`EmotionPipelineState::pending`]. The render-side
/// system (Phase 7.2) reads this resource and applies the
/// emotions to the `VrmModel`.
pub fn apply_emotions_system(
    mut ui_query: Query<&mut crate::component::ui::UiEmotionQueue, With<UiWindow>>,
    mut pipeline: ResMut<crate::resource::emotion_pipeline::EmotionPipelineState>,
) {
    let Some(mut queue) = ui_query.iter_mut().next() else {
        return;
    };
    while let Some(cmd) = queue.0.commands.pop_front() {
        pipeline.pending.push_back(cmd);
    }
}

#[cfg(test)]
#[path = "ui_consumers_tests.rs"]
mod tests;
