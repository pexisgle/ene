//! AI settings page.
//!
//! Provider / model / base-url / API key configuration, embedding
//! backend selection, memory toggle, the chat input box (Enter or
//! Send), and the "Latest Response" scroll area.
use super::input::SettingsInputState;
use super::widgets::SettingsAction;
use crate::ai_bridge::AiBridge;
use crate::character_state::AnimationControl;
use crate::component::ui::UiStateComponent;
use crate::settings::CharacterSettings;
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
use ene_core::UserInputResponse;
use ene_tool_proto::{MultiAnswer, QuestionItem};
use std::sync::Arc;

pub fn render(
    ui: &mut egui::Ui,
    settings: &mut CharacterSettings,
    _animation: &mut AnimationControl,
    ai: &Arc<AiBridge>,
    input: &mut SettingsInputState,
    world: &mut World,
    ui_entity: Entity,
) {
    let mut provider = settings
        .ai
        .ai
        .get_section::<ene_core::ProviderConfig>()
        .unwrap_or_default();
    let mut memory = settings
        .ai
        .ai
        .get_section::<ene_core::MemoryConfig>()
        .unwrap_or_default();

    ui.vertical(|ui| {
        ui.horizontal(|ui| {
            ui.label(crate::i18n::character_card());
            ui.add_sized(
                [220.0, 0.0],
                egui::Label::new(settings.current_character_card()),
            );
        });

        ui.horizontal(|ui| {
            ui.label(crate::i18n::user_name());
            let response = ui.add(
                egui::TextEdit::singleline(&mut input.ai_user_name).desired_width(f32::INFINITY),
            );
            if response.changed() {
                settings.ai.ai.user_name = input.ai_user_name.trim().to_string();
                settings.mark_dirty();
            }
        });

        ui.horizontal(|ui| {
            ui.label(crate::i18n::runtime_rules());
            let response = ui.add(
                egui::TextEdit::singleline(&mut input.ai_runtime_rules)
                    .desired_width(f32::INFINITY),
            );
            if response.changed() {
                settings.ai.ai.runtime_rules = input.ai_runtime_rules.clone();
                settings.mark_dirty();
            }
        });

        ui.horizontal(|ui| {
            ui.label(crate::i18n::provider_name());
            let response = ui.add(
                egui::TextEdit::singleline(&mut input.ai_provider_name)
                    .desired_width(f32::INFINITY),
            );
            if response.changed() {
                provider.name = input.ai_provider_name.trim().to_string();
                let _ = settings.ai.ai.set_section(&provider);
                settings.mark_dirty();
            }
        });

        ui.horizontal(|ui| {
            ui.label(crate::i18n::model());
            let response = ui
                .add(egui::TextEdit::singleline(&mut input.ai_model).desired_width(f32::INFINITY));
            if response.changed() {
                provider.model = input.ai_model.trim().to_string();
                let _ = settings.ai.ai.set_section(&provider);
                settings.mark_dirty();
            }
        });

        ui.horizontal(|ui| {
            ui.label(crate::i18n::base_url());
            let response = ui.add(
                egui::TextEdit::singleline(&mut input.ai_base_url).desired_width(f32::INFINITY),
            );
            if response.changed() {
                provider.base_url = input.ai_base_url.trim().to_string();
                let _ = settings.ai.ai.set_section(&provider);
                settings.mark_dirty();
            }
        });

        ui.horizontal(|ui| {
            ui.label(crate::i18n::api_key_source());
            let mut current_source = provider.api_key.source.clone();
            egui::ComboBox::from_id_salt("api_key_source")
                .selected_text(current_source.as_str())
                .show_ui(ui, |ui| {
                    ui.selectable_value(
                        &mut current_source,
                        "inline".to_string(),
                        crate::i18n::inline_settings(),
                    );
                    ui.selectable_value(
                        &mut current_source,
                        "env".to_string(),
                        crate::i18n::environment(),
                    );
                });
            if current_source != provider.api_key.source {
                provider.api_key.source = current_source;
                let _ = settings.ai.ai.set_section(&provider);
                settings.mark_dirty();
            }
        });

        if provider.api_key.source == "env" {
            ui.horizontal(|ui| {
                ui.label(crate::i18n::api_key_env_var());
                let response = ui.add(
                    egui::TextEdit::singleline(&mut input.ai_api_key_env)
                        .desired_width(f32::INFINITY),
                );
                if response.changed() {
                    provider.api_key.env = input.ai_api_key_env.trim().to_string();
                    let _ = settings.ai.ai.set_section(&provider);
                    settings.mark_dirty();
                }
            });
        } else {
            ui.horizontal(|ui| {
                ui.label(crate::i18n::api_key());
                let response = ui.add(
                    egui::TextEdit::singleline(&mut input.ai_api_key)
                        .password(true)
                        .desired_width(f32::INFINITY),
                );
                if response.changed() {
                    provider.api_key.inline = input.ai_api_key.trim().to_string();
                    let _ = settings.ai.ai.set_section(&provider);
                    settings.mark_dirty();
                }
            });
        }

        ui.horizontal(|ui| {
            ui.label(crate::i18n::chat_input());
            // Grey out the chat input + Send button while a
            // request is in flight, so the user cannot double-fire
            // a Run before the actor reports Done / Failed.
            let processing = ai.is_processing();
            ui.add_enabled_ui(!processing, |ui| {
                let response = ui.add(
                    egui::TextEdit::singleline(&mut input.ai_chat_input)
                        .desired_width(f32::INFINITY)
                        .hint_text(if processing {
                            crate::i18n::waiting_for_ai()
                        } else {
                            crate::i18n::message_to_ai()
                        }),
                );
                let send_clicked = ui
                    .add_enabled(!processing, egui::Button::new(crate::i18n::send()))
                    .clicked();
                if response.changed() {
                    if let Some(mut ui_state) = world.get_mut::<UiStateComponent>(ui_entity) {
                        ui_state.0.ai_chat_input = input.ai_chat_input.clone();
                    }
                    settings.mark_dirty();
                }
                let send_with_enter = !processing
                    && response.lost_focus()
                    && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if send_clicked || send_with_enter {
                    // Sync the in-memory text buffer, then send.
                    if let Some(mut ui_state) = world.get_mut::<UiStateComponent>(ui_entity) {
                        ui_state.0.ai_chat_input = input.ai_chat_input.clone();
                    }
                    let _ = SettingsAction::SendAiChat;
                    send_chat(settings, ai, world, ui_entity);
                    input.ai_chat_input.clear();
                }
            });
        });

        ui.separator();
        ui.label(crate::i18n::embedding_settings());

        ui.horizontal(|ui| {
            ui.label(crate::i18n::provider());
            let mut current_provider = input.ai_embedding_provider.clone();
            egui::ComboBox::from_id_salt("embedding_provider")
                .selected_text(current_provider.as_str())
                .show_ui(ui, |ui| {
                    ui.selectable_value(
                        &mut current_provider,
                        "cloud".to_string(),
                        crate::i18n::cloud_api(),
                    );
                    ui.selectable_value(
                        &mut current_provider,
                        "local".to_string(),
                        crate::i18n::local_gguf(),
                    );
                });
            if current_provider != input.ai_embedding_provider {
                input.ai_embedding_provider = current_provider.clone();
                provider.embedding.backend = current_provider.clone();
                if current_provider.as_str() == "local" {
                    provider.embedding.local.model = "jina-embeddings-v5-text-small".to_string();
                    input.ai_embedding_model = provider.embedding.local.model.clone();
                    input.ai_embedding_dimensions = "auto".to_string();
                } else {
                    provider.embedding.cloud.model = "text-embedding-3-small".to_string();
                    provider.embedding.cloud.dimensions = 1536;
                    input.ai_embedding_model = provider.embedding.cloud.model.clone();
                    input.ai_embedding_dimensions = provider.embedding.cloud.dimensions.to_string();
                }
                let _ = settings.ai.ai.set_section(&provider);
                settings.mark_dirty();
            }
        });

        ui.horizontal(|ui| {
            ui.label(crate::i18n::model());
            let response = ui.add(
                egui::TextEdit::singleline(&mut input.ai_embedding_model)
                    .desired_width(f32::INFINITY),
            );
            if response.changed() {
                if input.ai_embedding_provider == "local" {
                    provider.embedding.local.model = input.ai_embedding_model.clone();
                } else {
                    provider.embedding.cloud.model = input.ai_embedding_model.clone();
                }
                let _ = settings.ai.ai.set_section(&provider);
                settings.mark_dirty();
            }
        });

        ui.horizontal(|ui| {
            ui.label(crate::i18n::dimensions());
            if input.ai_embedding_provider == "local" {
                ui.add_sized(
                    [100.0, 0.0],
                    egui::Label::new(crate::i18n::auto_from_model()),
                );
            } else {
                let response = ui.add(
                    egui::TextEdit::singleline(&mut input.ai_embedding_dimensions)
                        .desired_width(100.0),
                );
                if response.changed()
                    && let Ok(dims) = input.ai_embedding_dimensions.parse::<usize>()
                {
                    provider.embedding.cloud.dimensions = dims;
                    let _ = settings.ai.ai.set_section(&provider);
                    settings.mark_dirty();
                }
            }
        });

        ui.separator();
        ui.label(crate::i18n::memory_settings());

        ui.horizontal(|ui| {
            let mut checked = input.ai_memory_enabled;
            ui.checkbox(&mut checked, crate::i18n::enable_long_term_memory());
            if checked != input.ai_memory_enabled {
                input.ai_memory_enabled = checked;
                memory.enabled = checked;
                let _ = settings.ai.ai.set_section(&memory);
                settings.mark_dirty();
            }
        });

        ui.separator();
        ui.label(crate::i18n::latest_response());
        let ai_latest_response = if let Some(ui_state) = world.get::<UiStateComponent>(ui_entity) {
            ui_state.0.ai_latest_response.clone()
        } else {
            String::new()
        };
        egui::ScrollArea::vertical()
            .max_height(180.0)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                if ai_latest_response.is_empty() {
                    ui.weak(crate::i18n::empty_parentheses());
                } else {
                    ui.label(ai_latest_response);
                }
            });

        // Top-level egui `Window`s so the dialogs pop over the
        // rest of the AI page. Both clear their `pending_*` state
        // on every button click (any answer, including Cancel,
        // ends the in-flight request). The data path is wired in
        // `runtime.rs::about_to_wait`.
        render_permission_dialog(ui, world, ui_entity, ai);
        render_user_input_dialog(ui, world, ui_entity, ai);
    });
}

/// Render the permission-request dialog when
/// `UiState::pending_permission` is `Some`. Yes / No /
/// Always each call `AiBridge::answer_permission` with the
/// matching `PermissionDecision` and clear the in-flight
/// state. The dialog always renders at the top of the
/// settings window — the runtime already forced the
/// settings window visible on `PermissionRequired`.
fn render_permission_dialog(
    ui: &mut egui::Ui,
    world: &mut World,
    ui_entity: Entity,
    ai: &Arc<AiBridge>,
) {
    let pending = world
        .get::<UiStateComponent>(ui_entity)
        .and_then(|s| s.0.pending_permission.clone());
    let Some(pending) = pending else {
        return;
    };
    // Clone the request_id up front so the window-close
    // handler can forward a "Deny" decision even if the
    // buttons already moved it.
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
                            ene_core::PermissionDecision::AllowOnce,
                        );
                        clear_pending_permission(world, ui_entity);
                    }
                    if ui.button(crate::i18n::no()).clicked() {
                        let _ = ai.answer_permission(
                            request_id.clone(),
                            ene_core::PermissionDecision::Deny,
                        );
                        clear_pending_permission(world, ui_entity);
                    }
                    if ui.button(crate::i18n::always()).clicked() {
                        let _ = ai.answer_permission(
                            request_id.clone(),
                            ene_core::PermissionDecision::AllowSession,
                        );
                        clear_pending_permission(world, ui_entity);
                    }
                });
            });
        });
    if !open {
        // Treat window-close as "Deny" so a dismissed dialog
        // does not stall the actor waiting on the oneshot.
        let _ = ai.answer_permission(request_id, ene_core::PermissionDecision::Deny);
        clear_pending_permission(world, ui_entity);
    }
}

fn clear_pending_permission(world: &mut World, ui_entity: Entity) {
    if let Some(mut ui_state) = world.get_mut::<UiStateComponent>(ui_entity) {
        ui_state.0.pending_permission = None;
    }
}

/// Render the user-input dialog when `UiState::pending_user_input`
/// is `Some`. One row per sub-question in the original prompt;
/// each row carries the predefined options as buttons + a
/// free-text `TextEdit` + a Skip checkbox. Submit packs the
/// answers into a `Vec<MultiAnswer>` in the same order as the
/// prompt and calls `AiBridge::answer_user_input` with
/// `UserInputResponse::Multi(..)`. Cancel submits
/// `UserInputResponse::Cancel`.
fn render_user_input_dialog(
    ui: &mut egui::Ui,
    world: &mut World,
    ui_entity: Entity,
    ai: &Arc<AiBridge>,
) {
    let snapshot = world.get::<UiStateComponent>(ui_entity).map(|s| {
        (
            s.0.pending_user_input.clone(),
            s.0.user_input_drafts.clone(),
        )
    });
    let Some((Some(prompt_snapshot), drafts)) = snapshot else {
        return;
    };
    if prompt_snapshot.prompt.items.len() != drafts.len() {
        // Defensive: bail out and let the next event re-populate.
        return;
    }
    // The closure body needs `&mut Vec<QuestionDraft>` for the
    // row widgets, so we move the cloned drafts into a
    // local mutable variable. The Submit handler re-reads the
    // mutated values back out of it.
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
                        clear_pending_user_input(world, ui_entity);
                    }
                    if ui.button(crate::i18n::cancel()).clicked() {
                        let _ = ai.answer_user_input(request_id.clone(), UserInputResponse::Cancel);
                        clear_pending_user_input(world, ui_entity);
                    }
                });
            });
        });
    if !open {
        // Window close = Cancel.
        let _ = ai.answer_user_input(request_id, UserInputResponse::Cancel);
        clear_pending_user_input(world, ui_entity);
    }
}

fn render_user_input_row(
    ui: &mut egui::Ui,
    index: usize,
    item: &QuestionItem,
    draft: &mut crate::settings::QuestionDraft,
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
                // Typing in the free-text field counts as
                // engagement: clear any selected option so Submit
                // packs an `Answer` rather than a `Selected`.
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

fn clear_pending_user_input(world: &mut World, ui_entity: Entity) {
    if let Some(mut ui_state) = world.get_mut::<UiStateComponent>(ui_entity) {
        ui_state.0.pending_user_input = None;
        ui_state.0.user_input_drafts.clear();
    }
}

fn send_chat(
    _settings: &mut CharacterSettings,
    ai: &Arc<AiBridge>,
    world: &mut World,
    ui_entity: Entity,
) {
    if let Some(mut ui_state) = world.get_mut::<UiStateComponent>(ui_entity) {
        let trimmed = ui_state.0.ai_chat_input.trim().to_string();
        if trimmed.is_empty() {
            return;
        }
        ai.run(trimmed);
        ui_state.0.ai_chat_input.clear();
        ui_state.0.ai_latest_response.clear();
    }
}
