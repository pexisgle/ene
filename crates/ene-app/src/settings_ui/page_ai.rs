use bevy::prelude::*;
use bevy_egui::egui;
use crate::app_config::CharacterSettings;
use crate::character::CharacterAnimationControl;
use crate::ai_bridge::AiRequestEvent;
use super::{
    SettingsButtonAction, SettingsInputState, SettingsValueKind,
    widgets::apply_action,
};

pub fn render_ai_page(
    ui: &mut egui::Ui,
    settings: &mut CharacterSettings,
    animation_control: &mut CharacterAnimationControl,
    ai_request_writer: &mut MessageWriter<AiRequestEvent>,
    input_state: &mut SettingsInputState,
) {
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
                egui::TextEdit::singleline(&mut input_state.ai_user_name)
                    .desired_width(f32::INFINITY),
            );
            if response.changed() {
                let _ = SettingsValueKind::AiUserName
                    .apply_input(input_state.ai_user_name.trim(), settings);
            }
        });

        ui.horizontal(|ui| {
            ui.label("Runtime Rules");
            let response = ui.add(
                egui::TextEdit::singleline(&mut input_state.ai_runtime_rules)
                    .desired_width(f32::INFINITY),
            );
            if response.changed() {
                let _ = SettingsValueKind::AiRuntimeRules
                    .apply_input(&input_state.ai_runtime_rules, settings);
            }
        });

        ui.horizontal(|ui| {
            ui.label("Provider Name");
            ui.add_sized(
                [280.0, 0.0],
                egui::Label::new(
                    SettingsValueKind::AiProviderName.current_text(settings, animation_control),
                ),
            );
        });

        ui.horizontal(|ui| {
            ui.label("Model");
            ui.add_sized(
                [280.0, 0.0],
                egui::Label::new(
                    SettingsValueKind::AiModel.current_text(settings, animation_control),
                ),
            );
        });

        ui.horizontal(|ui| {
            ui.label("Base URL");
            let response = ui.add(
                egui::TextEdit::singleline(&mut input_state.ai_base_url)
                    .desired_width(f32::INFINITY),
            );
            if response.changed() {
                let _ = SettingsValueKind::AiBaseUrl
                    .apply_input(input_state.ai_base_url.trim(), settings);
            }
        });

        ui.horizontal(|ui| {
            ui.label("API Key");
            let response = ui.add(
                egui::TextEdit::singleline(&mut input_state.ai_api_key)
                    .password(true)
                    .desired_width(f32::INFINITY),
            );
            if response.changed() {
                let _ = SettingsValueKind::AiApiKey
                    .apply_input(input_state.ai_api_key.trim(), settings);
            }
        });

        ui.horizontal(|ui| {
            ui.label("Chat Input");
            let response = ui.add(
                egui::TextEdit::singleline(&mut input_state.ai_chat_input)
                    .desired_width(f32::INFINITY)
                    .hint_text("message to AI"),
            );
            let send_clicked = ui.button("Send").clicked();
            if response.changed() {
                let _ = SettingsValueKind::AiChatInput
                    .apply_input(input_state.ai_chat_input.as_str(), settings);
            }
            let send_with_enter =
                response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if send_clicked || send_with_enter {
                let _ = SettingsValueKind::AiChatInput
                    .apply_input(input_state.ai_chat_input.as_str(), settings);
                apply_action(
                    SettingsButtonAction::SendAiChat,
                    settings,
                    animation_control,
                    ai_request_writer,
                );
                input_state.ai_chat_input.clear();
            }
        });

        ui.separator();
        ui.label("Embedding Settings");

        ui.horizontal(|ui| {
            ui.label("Provider");
            let mut current_provider = input_state.ai_embedding_provider.clone();
            egui::ComboBox::from_id_salt("embedding_provider")
                .selected_text(&current_provider)
                .show_ui(ui, |ui| {
                    ui.selectable_value(
                        &mut current_provider,
                        "api".to_string(),
                        "API (OpenAI-compatible)",
                    );
                    ui.selectable_value(
                        &mut current_provider,
                        "local".to_string(),
                        "Local (GGUF / Candle)",
                    );
                });
            if current_provider != input_state.ai_embedding_provider {
                input_state.ai_embedding_provider = current_provider.clone();
                settings.ai.ai.embedding.provider_type = match current_provider.as_str() {
                    "local" => ene_config::EmbeddingProviderType::Local,
                    _ => ene_config::EmbeddingProviderType::Api,
                };
                match current_provider.as_str() {
                    "local" => {
                        settings.ai.ai.embedding.model =
                            "jina-embeddings-v5-text-nano".to_string();
                        settings.ai.ai.embedding.dimensions = None;
                        input_state.ai_embedding_model = settings.ai.ai.embedding.model.clone();
                        input_state.ai_embedding_dimensions = "auto".to_string();
                    }
                    _ => {
                        settings.ai.ai.embedding.model = "text-embedding-3-small".to_string();
                        settings.ai.ai.embedding.dimensions = Some(1536);
                        input_state.ai_embedding_model = settings.ai.ai.embedding.model.clone();
                        input_state.ai_embedding_dimensions = settings
                            .ai
                            .ai
                            .embedding
                            .dimensions
                            .map(|d| d.to_string())
                            .unwrap_or_default();
                    }
                }
            }
        });

        ui.horizontal(|ui| {
            ui.label("Model");
            let response = ui.add(
                egui::TextEdit::singleline(&mut input_state.ai_embedding_model)
                    .desired_width(f32::INFINITY),
            );
            if response.changed() {
                settings.ai.ai.embedding.model = input_state.ai_embedding_model.clone();
            }
        });

        ui.horizontal(|ui| {
            ui.label("Base URL");
            let response = ui.add(
                egui::TextEdit::singleline(&mut input_state.ai_embedding_base_url)
                    .desired_width(f32::INFINITY),
            );
            if response.changed() {
                settings.ai.ai.embedding.base_url = input_state.ai_embedding_base_url.clone();
            }
        });

        ui.horizontal(|ui| {
            ui.label("Dimensions");
            if input_state.ai_embedding_provider == "local" {
                ui.add_sized([100.0, 0.0], egui::Label::new("auto (from model)"));
            } else {
                let response = ui.add(
                    egui::TextEdit::singleline(&mut input_state.ai_embedding_dimensions)
                        .desired_width(100.0),
                );
                if response.changed() {
                    if let Ok(dims) = input_state.ai_embedding_dimensions.parse::<usize>() {
                        settings.ai.ai.embedding.dimensions = Some(dims);
                    }
                }
            }
        });

        ui.separator();
        ui.label("Memory Settings");

        ui.horizontal(|ui| {
            let mut checked = input_state.ai_memory_enabled;
            ui.checkbox(&mut checked, "Enable Long-term Memory");
            if checked != input_state.ai_memory_enabled {
                input_state.ai_memory_enabled = checked;
                settings.ai.ai.memory.enabled = checked;
            }
        });

        ui.separator();
        ui.label("Latest Response");
        egui::ScrollArea::vertical()
            .max_height(180.0)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                if settings.ui.ai_latest_response.is_empty() {
                    ui.weak("(empty)");
                } else {
                    ui.label(&settings.ui.ai_latest_response);
                }
            });
    });
}
