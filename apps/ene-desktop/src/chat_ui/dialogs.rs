//! Permission and user-input dialogs for the chat window.

use std::sync::Arc;

use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
use ene_runtime::UserInputResponse;
use ene_tool_proto::{MultiAnswer, QuestionItem};

use crate::ai_bridge::AiBridge;
use crate::component::chat::ChatStateComponent;
use crate::settings::QuestionDraft;

pub fn render_permission_dialog(
    ui: &mut egui::Ui,
    world: &mut World,
    chat_entity: Entity,
    ai: &Arc<AiBridge>,
) {
    let pending = world
        .get::<ChatStateComponent>(chat_entity)
        .and_then(|s| s.0.pending_permission.clone());
    let Some(pending) = pending else {
        return;
    };
    let request_id = pending.request_id;
    let mut open = true;
    egui::Window::new(crate::i18n::permission_requested())
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ui.ctx(), |ui| {
            ui.vertical(|ui| {
                ui.label(format!(
                    "{}: {}",
                    crate::i18n::action_label(),
                    pending.action
                ));
                ui.label(format!(
                    "{}: {}",
                    crate::i18n::target_label(),
                    pending.target
                ));
                if let Some(description) = &pending.description {
                    ui.label(format!(
                        "{}: {}",
                        crate::i18n::description_label(),
                        description
                    ));
                }
                ui.separator();
                ui.horizontal(|ui| {
                    if ui.button(crate::i18n::yes()).clicked() {
                        let _ = ai.answer_permission(
                            request_id.clone(),
                            ene_runtime::PermissionDecision::AllowOnce,
                        );
                        clear_pending_permission(world, chat_entity);
                    }
                    if ui.button(crate::i18n::no()).clicked() {
                        let _ = ai.answer_permission(
                            request_id.clone(),
                            ene_runtime::PermissionDecision::Deny,
                        );
                        clear_pending_permission(world, chat_entity);
                    }
                    if ui.button(crate::i18n::always()).clicked() {
                        let _ = ai.answer_permission(
                            request_id.clone(),
                            ene_runtime::PermissionDecision::AllowSession,
                        );
                        clear_pending_permission(world, chat_entity);
                    }
                });
            });
        });
    if !open {
        let _ = ai.answer_permission(request_id, ene_runtime::PermissionDecision::Deny);
        clear_pending_permission(world, chat_entity);
    }
}

pub fn render_user_input_dialog(
    ui: &mut egui::Ui,
    world: &mut World,
    chat_entity: Entity,
    ai: &Arc<AiBridge>,
) {
    let snapshot = world.get::<ChatStateComponent>(chat_entity).map(|s| {
        (
            s.0.pending_user_input.clone(),
            s.0.user_input_drafts.clone(),
        )
    });
    let Some((Some(prompt_snapshot), drafts)) = snapshot else {
        return;
    };
    if prompt_snapshot.prompt.items.len() != drafts.len() {
        return;
    }
    let mut drafts = drafts;
    let request_id = prompt_snapshot.request_id;
    let items = prompt_snapshot.prompt.items.clone();
    let mut open = true;
    egui::Window::new(crate::i18n::user_input_requested())
        .open(&mut open)
        .collapsible(false)
        .resizable(true)
        .default_size([420.0, 280.0])
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ui.ctx(), |ui| {
            ui.vertical(|ui| {
                ui.label(crate::i18n::answer_each_question());
                ui.separator();
                for (i, (item, draft)) in items.iter().zip(drafts.iter_mut()).enumerate() {
                    render_user_input_row(ui, i, item, draft);
                }
                ui.separator();
                ui.horizontal(|ui| {
                    if ui.button(crate::i18n::submit()).clicked() {
                        let answers: Vec<MultiAnswer> = drafts
                            .iter()
                            .map(|d| {
                                if d.skipped {
                                    MultiAnswer::Skip
                                } else if let Some(option) = &d.selected {
                                    MultiAnswer::Selected {
                                        option: option.clone(),
                                    }
                                } else {
                                    MultiAnswer::Answer {
                                        text: d.text.clone(),
                                    }
                                }
                            })
                            .collect();
                        let _ = ai.answer_user_input(
                            request_id.clone(),
                            UserInputResponse::Multi(answers),
                        );
                        clear_pending_user_input(world, chat_entity);
                    }
                    if ui.button(crate::i18n::cancel()).clicked() {
                        let _ = ai.answer_user_input(request_id.clone(), UserInputResponse::Cancel);
                        clear_pending_user_input(world, chat_entity);
                    }
                });
            });
        });
    if !open {
        let _ = ai.answer_user_input(request_id, UserInputResponse::Cancel);
        clear_pending_user_input(world, chat_entity);
    }
}

fn render_user_input_row(
    ui: &mut egui::Ui,
    index: usize,
    item: &QuestionItem,
    draft: &mut QuestionDraft,
) {
    ui.collapsing(format!("{}. {}", index + 1, item.question), |ui| {
        if !item.options.is_empty() {
            ui.horizontal_wrapped(|ui| {
                for option in &item.options {
                    let selected = draft.selected.as_deref() == Some(option.as_str());
                    if ui.selectable_label(selected, option).clicked() {
                        draft.selected = Some(option.clone());
                        draft.skipped = false;
                    }
                }
            });
        }
        if item.allow_free_text {
            let response =
                ui.add(egui::TextEdit::singleline(&mut draft.text).desired_width(f32::INFINITY));
            if response.changed() && !draft.text.is_empty() {
                draft.selected = None;
                draft.skipped = false;
            }
        }
        let mut skipped = draft.skipped;
        if ui
            .checkbox(&mut skipped, crate::i18n::skip_this_question())
            .changed()
        {
            draft.skipped = skipped;
        }
    });
}

fn clear_pending_permission(world: &mut World, chat_entity: Entity) {
    if let Some(mut chat) = world.get_mut::<ChatStateComponent>(chat_entity) {
        chat.0.pending_permission = None;
    }
}

fn clear_pending_user_input(world: &mut World, chat_entity: Entity) {
    if let Some(mut chat) = world.get_mut::<ChatStateComponent>(chat_entity) {
        chat.0.pending_user_input = None;
        chat.0.user_input_drafts.clear();
    }
}
