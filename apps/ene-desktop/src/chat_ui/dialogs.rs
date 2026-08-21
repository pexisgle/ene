use std::sync::Arc;

use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;

use crate::component::chat::ChatStateComponent;
use crate::core_session::CoreSession;
use crate::settings::QuestionDraft;

const PERMISSION_FIELD_MAX_CHARS: usize = 160;
const PERMISSION_FIELD_MAX_LINES: usize = 6;

fn truncate_permission_field(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let capped = if lines.len() > PERMISSION_FIELD_MAX_LINES {
        format!("{}\n...", lines[..PERMISSION_FIELD_MAX_LINES].join("\n"))
    } else {
        text.to_string()
    };
    truncate_chars(&capped, PERMISSION_FIELD_MAX_CHARS)
}

fn truncate_chars(text: &str, max_chars: usize) -> String {
    let count = text.chars().count();
    if count <= max_chars {
        return text.to_string();
    }
    let truncated: String = text.chars().take(max_chars).collect();
    format!("{truncated}...")
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
    ai: &Arc<CoreSession>,
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
                    ai.answer_permission(request_id.clone(), "allow");
                    clear_pending_permission(world, chat_entity);
                }
                if ui
                    .button(i18n_embed_fl::fl!(crate::i18n::loader(), "no"))
                    .clicked()
                {
                    ai.answer_permission(request_id.clone(), "deny");
                    clear_pending_permission(world, chat_entity);
                }
                if ui
                    .button(i18n_embed_fl::fl!(crate::i18n::loader(), "always"))
                    .clicked()
                {
                    ai.answer_permission(request_id.clone(), "allow");
                    clear_pending_permission(world, chat_entity);
                }
            });
        });
    });
    if !open {
        ai.answer_permission(request_id, "deny");
        clear_pending_permission(world, chat_entity);
    }
}

pub fn render_user_input_dialog(
    ui: &mut egui::Ui,
    world: &mut World,
    chat_entity: Entity,
    ai: &Arc<CoreSession>,
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
    if prompt_snapshot.prompt.questions.len() != drafts.len() {
        return;
    }
    let mut drafts = drafts;
    let request_id = prompt_snapshot.request_id;
    let questions = prompt_snapshot.prompt.questions;
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
            if !prompt_snapshot.prompt.title.is_empty() {
                ui.strong(&prompt_snapshot.prompt.title);
            }
            ui.label(i18n_embed_fl::fl!(
                crate::i18n::loader(),
                "answer-each-question"
            ));
            ui.separator();
            for (i, (question, draft)) in questions.iter().zip(drafts.iter_mut()).enumerate() {
                render_user_input_row(ui, i, question, draft);
            }
            ui.separator();
            ui.horizontal(|ui| {
                if ui
                    .button(i18n_embed_fl::fl!(crate::i18n::loader(), "submit"))
                    .clicked()
                {
                    let text = drafts
                        .iter()
                        .filter(|draft| !draft.skipped)
                        .map(|draft| draft.selected.clone().unwrap_or_else(|| draft.text.clone()))
                        .collect::<Vec<_>>()
                        .join("\n");
                    ai.answer_user_input(request_id.clone(), text);
                    clear_pending_user_input(world, chat_entity);
                }
                if ui
                    .button(i18n_embed_fl::fl!(crate::i18n::loader(), "cancel"))
                    .clicked()
                {
                    clear_pending_user_input(world, chat_entity);
                }
            });
        });
    });
    if !open {
        clear_pending_user_input(world, chat_entity);
    }
}

fn render_user_input_row(
    ui: &mut egui::Ui,
    index: usize,
    question: &str,
    draft: &mut QuestionDraft,
) {
    ui.collapsing(format!("{}. {question}", index + 1), |ui| {
        let response =
            ui.add(egui::TextEdit::singleline(&mut draft.text).desired_width(f32::INFINITY));
        if response.changed() && !draft.text.is_empty() {
            draft.selected = None;
            draft.skipped = false;
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
        let long = "あ".repeat(PERMISSION_FIELD_MAX_CHARS + 8);
        let truncated = truncate_permission_field(&long);
        assert!(truncated.ends_with("..."));
        assert_eq!(truncated.chars().count(), PERMISSION_FIELD_MAX_CHARS + 3);
    }
}
