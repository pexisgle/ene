//! Permission and user-input dialogs for the chat window.

use std::sync::Arc;

use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
use ene_plugin_proto::{MultiAnswer, QuestionItem};
use ene_runtime::UserInputResponse;
use ene_util::truncate::Truncate;

use crate::ai_bridge::AiBridge;
use crate::component::chat::ChatStateComponent;
use crate::settings::QuestionDraft;

/// Hard cap so long shell commands / paths cannot push the dialog
/// off-screen. Applied to every permission field before display.
const PERMISSION_FIELD_MAX_CHARS: usize = 160;
/// Secondary cap for multi-line payloads (e.g. heredoc-style commands).
const PERMISSION_FIELD_MAX_LINES: usize = 6;

/// Truncate permission dialog field text for display.
///
/// Caps both line count and Unicode character count, appending `...`
/// when either limit is exceeded.
fn truncate_permission_field(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let capped = if lines.len() > PERMISSION_FIELD_MAX_LINES {
        format!("{}\n...", lines[..PERMISSION_FIELD_MAX_LINES].join("\n"))
    } else {
        text.to_string()
    };
    Truncate::simple(&capped, PERMISSION_FIELD_MAX_CHARS)
}

fn permission_field_label(ui: &mut egui::Ui, label: &str, value: &str) {
    ui.add(
        egui::Label::new(format!("{label}: {}", truncate_permission_field(value)))
            .wrap()
            .selectable(true),
    );
}

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
    egui::Window::new(i18n_embed_fl::fl!(
        crate::i18n::loader(),
        "permission-requested"
    ))
    .open(&mut open)
    .collapsible(false)
    .resizable(false)
    .default_width(420.0)
    .max_width(480.0)
    .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
    .show(ui.ctx(), |ui| {
        ui.vertical(|ui| {
            ui.set_max_width(440.0);
            permission_field_label(
                ui,
                &i18n_embed_fl::fl!(crate::i18n::loader(), "action-label"),
                &pending.action,
            );
            permission_field_label(
                ui,
                &i18n_embed_fl::fl!(crate::i18n::loader(), "target-label"),
                &pending.target,
            );
            if let Some(description) = &pending.description {
                permission_field_label(
                    ui,
                    &i18n_embed_fl::fl!(crate::i18n::loader(), "description-label"),
                    description,
                );
            }
            ui.separator();
            ui.horizontal(|ui| {
                if ui
                    .button(i18n_embed_fl::fl!(crate::i18n::loader(), "yes"))
                    .clicked()
                {
                    drop(ai.answer_permission(
                        request_id.clone(),
                        ene_runtime::PermissionDecision::AllowOnce,
                    ));
                    clear_pending_permission(world, chat_entity);
                }
                if ui
                    .button(i18n_embed_fl::fl!(crate::i18n::loader(), "no"))
                    .clicked()
                {
                    drop(ai.answer_permission(
                        request_id.clone(),
                        ene_runtime::PermissionDecision::Deny,
                    ));
                    clear_pending_permission(world, chat_entity);
                }
                if ui
                    .button(i18n_embed_fl::fl!(crate::i18n::loader(), "always"))
                    .clicked()
                {
                    drop(ai.answer_permission(
                        request_id.clone(),
                        ene_runtime::PermissionDecision::AllowSession,
                    ));
                    clear_pending_permission(world, chat_entity);
                }
            });
        });
    });
    if !open {
        drop(ai.answer_permission(request_id, ene_runtime::PermissionDecision::Deny));
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
    let items = prompt_snapshot.prompt.items;
    let mut open = true;
    egui::Window::new(i18n_embed_fl::fl!(
        crate::i18n::loader(),
        "user-input-requested"
    ))
    .open(&mut open)
    .collapsible(false)
    .resizable(true)
    .default_size([420.0, 280.0])
    .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
    .show(ui.ctx(), |ui| {
        ui.vertical(|ui| {
            ui.label(i18n_embed_fl::fl!(
                crate::i18n::loader(),
                "answer-each-question"
            ));
            ui.separator();
            for (i, (item, draft)) in items.iter().zip(drafts.iter_mut()).enumerate() {
                render_user_input_row(ui, i, item, draft);
            }
            ui.separator();
            ui.horizontal(|ui| {
                if ui
                    .button(i18n_embed_fl::fl!(crate::i18n::loader(), "submit"))
                    .clicked()
                {
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
                    drop(
                        ai.answer_user_input(request_id.clone(), UserInputResponse::Multi(answers)),
                    );
                    clear_pending_user_input(world, chat_entity);
                }
                if ui
                    .button(i18n_embed_fl::fl!(crate::i18n::loader(), "cancel"))
                    .clicked()
                {
                    drop(ai.answer_user_input(request_id.clone(), UserInputResponse::Cancel));
                    clear_pending_user_input(world, chat_entity);
                }
            });
        });
    });
    if !open {
        drop(ai.answer_user_input(request_id, UserInputResponse::Cancel));
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
            .checkbox(
                &mut skipped,
                i18n_embed_fl::fl!(crate::i18n::loader(), "skip-this-question"),
            )
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_field_is_unchanged() {
        assert_eq!(truncate_permission_field("ls -la"), "ls -la");
    }

    #[test]
    fn long_field_is_char_truncated() {
        let long = "a".repeat(PERMISSION_FIELD_MAX_CHARS + 40);
        let truncated = truncate_permission_field(&long);
        assert!(truncated.ends_with("..."));
        assert_eq!(
            truncated.chars().count(),
            PERMISSION_FIELD_MAX_CHARS + 3,
            "max chars plus ellipsis"
        );
    }

    #[test]
    fn many_lines_are_line_truncated() {
        let many_lines = (0..PERMISSION_FIELD_MAX_LINES + 4)
            .map(|i| format!("line-{i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let truncated = truncate_permission_field(&many_lines);
        assert!(truncated.ends_with("..."));
        assert_eq!(
            truncated.lines().count(),
            PERMISSION_FIELD_MAX_LINES + 1,
            "capped lines plus ellipsis line"
        );
        assert!(truncated.contains("line-0"));
        assert!(!truncated.contains(&format!("line-{PERMISSION_FIELD_MAX_LINES}")));
    }

    #[test]
    fn unicode_char_limit_is_respected() {
        let long = "あ".repeat(PERMISSION_FIELD_MAX_CHARS + 10);
        let truncated = truncate_permission_field(&long);
        assert!(truncated.ends_with("..."));
        assert_eq!(truncated.chars().count(), PERMISSION_FIELD_MAX_CHARS + 3);
    }
}
