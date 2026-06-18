//! AI settings page.
//!
//! Provider / model / base-url / API key configuration, embedding
//! backend selection, memory toggle, the chat input box (Enter or
//! Send), and the "Latest Response" scroll area.
use super::input::SettingsInputState;
use super::widgets::SettingsAction;
use crate::ai_bridge::AiBridge;
use crate::character_state::AnimationControl;
use crate::settings::CharacterSettings;
use std::sync::Arc;

pub fn render(
    ui: &mut egui::Ui,
    settings: &mut CharacterSettings,
    _animation: &mut AnimationControl,
    ai: &Arc<AiBridge>,
    input: &mut SettingsInputState,
    world: &mut hecs::World,
    ui_entity: hecs::Entity,
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
            ui.label("Character Card");
            ui.add_sized(
                [220.0, 0.0],
                egui::Label::new(settings.current_character_card()),
            );
        });

        ui.horizontal(|ui| {
            ui.label("User Name");
            let response = ui.add(
                egui::TextEdit::singleline(&mut input.ai_user_name).desired_width(f32::INFINITY),
            );
            if response.changed() {
                settings.ai.ai.user_name = input.ai_user_name.trim().to_string();
                settings.mark_dirty();
            }
        });

        ui.horizontal(|ui| {
            ui.label("Runtime Rules");
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
            ui.label("Provider Name");
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
            ui.label("Model");
            let response = ui
                .add(egui::TextEdit::singleline(&mut input.ai_model).desired_width(f32::INFINITY));
            if response.changed() {
                provider.model = input.ai_model.trim().to_string();
                let _ = settings.ai.ai.set_section(&provider);
                settings.mark_dirty();
            }
        });

        ui.horizontal(|ui| {
            ui.label("Base URL");
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
            ui.label("API Key Source");
            let mut current_source = provider.api_key.source.clone();
            egui::ComboBox::from_id_salt("api_key_source")
                .selected_text(current_source.as_str())
                .show_ui(ui, |ui| {
                    ui.selectable_value(
                        &mut current_source,
                        "inline".to_string(),
                        "Inline (settings.json)",
                    );
                    ui.selectable_value(&mut current_source, "env".to_string(), "Environment");
                });
            if current_source != provider.api_key.source {
                provider.api_key.source = current_source;
                let _ = settings.ai.ai.set_section(&provider);
                settings.mark_dirty();
            }
        });

        if provider.api_key.source == "env" {
            ui.horizontal(|ui| {
                ui.label("API Key Env Var");
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
                ui.label("API Key");
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
            ui.label("Chat Input");
            let response = ui.add(
                egui::TextEdit::singleline(&mut input.ai_chat_input)
                    .desired_width(f32::INFINITY)
                    .hint_text("message to AI"),
            );
            let send_clicked = ui.button("Send").clicked();
            if response.changed() {
                if let Ok(mut ui_state) = world.get::<&mut crate::settings::UiState>(ui_entity) {
                    ui_state.ai_chat_input = input.ai_chat_input.clone();
                }
                settings.mark_dirty();
            }
            let send_with_enter =
                response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if send_clicked || send_with_enter {
                // Sync the in-memory text buffer, then send.
                if let Ok(mut ui_state) = world.get::<&mut crate::settings::UiState>(ui_entity) {
                    ui_state.ai_chat_input = input.ai_chat_input.clone();
                }
                let _ = SettingsAction::SendAiChat; // silence unused import in narrow builds
                send_chat(settings, ai, world, ui_entity);
                input.ai_chat_input.clear();
            }
        });

        ui.separator();
        ui.label("Embedding Settings");

        ui.horizontal(|ui| {
            ui.label("Provider");
            let mut current_provider = input.ai_embedding_provider.clone();
            egui::ComboBox::from_id_salt("embedding_provider")
                .selected_text(current_provider.as_str())
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut current_provider, "cloud".to_string(), "Cloud (API)");
                    ui.selectable_value(&mut current_provider, "local".to_string(), "Local (GGUF)");
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
            ui.label("Model");
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
            ui.label("Dimensions");
            if input.ai_embedding_provider == "local" {
                ui.add_sized([100.0, 0.0], egui::Label::new("auto (from model)"));
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
        ui.label("Memory Settings");

        ui.horizontal(|ui| {
            let mut checked = input.ai_memory_enabled;
            ui.checkbox(&mut checked, "Enable Long-term Memory");
            if checked != input.ai_memory_enabled {
                input.ai_memory_enabled = checked;
                memory.enabled = checked;
                let _ = settings.ai.ai.set_section(&memory);
                settings.mark_dirty();
            }
        });

        ui.separator();
        ui.label("Latest Response");
        let ai_latest_response =
            if let Ok(ui_state) = world.get::<&crate::settings::UiState>(ui_entity) {
                ui_state.ai_latest_response.clone()
            } else {
                String::new()
            };
        egui::ScrollArea::vertical()
            .max_height(180.0)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                if ai_latest_response.is_empty() {
                    ui.weak("(empty)");
                } else {
                    ui.label(ai_latest_response);
                }
            });
    });
}

fn send_chat(
    _settings: &mut CharacterSettings,
    ai: &Arc<AiBridge>,
    world: &mut hecs::World,
    ui_entity: hecs::Entity,
) {
    if let Ok(mut ui_state) = world.get::<&mut crate::settings::UiState>(ui_entity) {
        let trimmed = ui_state.ai_chat_input.trim().to_string();
        if trimmed.is_empty() {
            return;
        }
        ai.run(trimmed);
        ui_state.ai_chat_input.clear();
        ui_state.ai_latest_response.clear();
    }
}
