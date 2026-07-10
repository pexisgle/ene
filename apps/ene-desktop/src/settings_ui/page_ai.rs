//! AI settings page — provider, embedding, and memory configuration.
//!
//! Chat lives in the dedicated chat window (F2); see #109.

use super::input::SettingsInputState;
use crate::ai_bridge::AiBridge;
use crate::character_state::AnimationControl;
use crate::settings::CharacterSettings;
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
use std::sync::Arc;

pub fn render(
    ui: &mut egui::Ui,
    settings: &mut CharacterSettings,
    _animation: &mut AnimationControl,
    _ai: &Arc<AiBridge>,
    input: &mut SettingsInputState,
    _world: &mut World,
    _ui_entity: Entity,
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
        ui.weak(crate::i18n::chat_open_hint());
        ui.separator();

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
    });
}
