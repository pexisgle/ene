use super::{SettingsButtonAction, SettingsInputState, SettingsValueKind, widgets::apply_action};
use crate::ai_bridge::EneRequestEvent;
use crate::app_config::CharacterSettings;
use crate::character::CharacterAnimationControl;
use bevy::prelude::*;
use bevy_egui::egui;
use ene_core::MemoryConfig;

pub fn render_ai_page(
    ui: &mut egui::Ui,
    settings: &mut CharacterSettings,
    animation_control: &mut CharacterAnimationControl,
    ai_request_writer: &mut MessageWriter<EneRequestEvent>,
    input_state: &mut SettingsInputState,
) {
    let mut provider_config = settings
        .ai
        .ai
        .get_section::<ene_core::ProviderConfig>()
        .unwrap_or_default();
    let mut mem_config = settings
        .ai
        .ai
        .get_section::<MemoryConfig>()
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
            let response = ui.add(
                egui::TextEdit::singleline(&mut input_state.ai_provider_name)
                    .desired_width(f32::INFINITY),
            );
            if response.changed() {
                let _ = SettingsValueKind::AiProviderName
                    .apply_input(input_state.ai_provider_name.trim(), settings);
            }
        });

        ui.horizontal(|ui| {
            ui.label("Model");
            let response = ui.add(
                egui::TextEdit::singleline(&mut input_state.ai_model)
                    .desired_width(f32::INFINITY),
            );
            if response.changed() {
                let _ = SettingsValueKind::AiModel
                    .apply_input(input_state.ai_model.trim(), settings);
            }
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
            ui.label("API Key Source");
            let mut current_source = provider_config.api_key.source.clone();
            egui::ComboBox::from_id_salt("api_key_source")
                .selected_text(&current_source)
                .show_ui(ui, |ui| {
                    ui.selectable_value(
                        &mut current_source,
                        "inline".to_string(),
                        "Inline (settings.json 平文)",
                    );
                    ui.selectable_value(
                        &mut current_source,
                        "env".to_string(),
                        "Environment (環境変数)",
                    );
                });
            if current_source != provider_config.api_key.source {
                provider_config.api_key.source = current_source;
                let _ = settings.ai.ai.set_section(&provider_config);
                settings.mark_dirty();
            }
        });

        if provider_config.api_key.source == "env" {
            ui.horizontal(|ui| {
                ui.label("API Key Env Var");
                let response = ui.add(
                    egui::TextEdit::singleline(&mut input_state.ai_api_key_env)
                        .desired_width(f32::INFINITY),
                );
                if response.changed() {
                    let _ = SettingsValueKind::AiApiKeyEnv
                        .apply_input(input_state.ai_api_key_env.trim(), settings);
                }
            });
        } else {
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
        }

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
                        "cloud".to_string(),
                        "Cloud (API-compatible)",
                    );
                    ui.selectable_value(
                        &mut current_provider,
                        "local".to_string(),
                        "Local (GGUF / Candle)",
                    );
                });
            if current_provider != input_state.ai_embedding_provider {
                input_state.ai_embedding_provider = current_provider.clone();
                provider_config.embedding.backend = current_provider.clone();
                if current_provider.as_str() == "local" {
                    provider_config.embedding.local.model = "jina-embeddings-v5-text-small".to_string();
                    input_state.ai_embedding_model = provider_config.embedding.local.model.clone();
                    input_state.ai_embedding_dimensions = "auto".to_string();
                } else {
                    provider_config.embedding.cloud.model = "text-embedding-3-small".to_string();
                    provider_config.embedding.cloud.dimensions = 1536;
                    input_state.ai_embedding_model = provider_config.embedding.cloud.model.clone();
                    input_state.ai_embedding_dimensions =
                        provider_config.embedding.cloud.dimensions.to_string();
                }
                let _ = settings.ai.ai.set_section(&provider_config);
                settings.mark_dirty();
            }
        });

        ui.horizontal(|ui| {
            ui.label("Model");
            let response = ui.add(
                egui::TextEdit::singleline(&mut input_state.ai_embedding_model)
                    .desired_width(f32::INFINITY),
            );
            if response.changed() {
                if input_state.ai_embedding_provider == "local" {
                    provider_config.embedding.local.model = input_state.ai_embedding_model.clone();
                    let _ = settings.ai.ai.set_section(&provider_config);
                } else {
                    provider_config.embedding.cloud.model = input_state.ai_embedding_model.clone();
                    let _ = settings.ai.ai.set_section(&provider_config);
                }
                settings.mark_dirty();
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
                if response.changed()
                    && let Ok(dims) = input_state.ai_embedding_dimensions.parse::<usize>()
                {
                    provider_config.embedding.cloud.dimensions = dims;
                    let _ = settings.ai.ai.set_section(&provider_config);
                    settings.mark_dirty();
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
                mem_config.enabled = checked;
                let _ = settings.ai.ai.set_section(&mem_config);
                settings.mark_dirty();
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
