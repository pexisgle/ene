//! AI settings page — embedding, memory, and proactive speech configuration.
//!
//! Chat lives in the dedicated chat window (F2); see #109.

use super::input::SettingsInputState;
use crate::ai_bridge::AiBridge;
use crate::character_state::AnimationControl;
use crate::settings::CharacterSettings;
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
use ene_ai::{AiConfig, AiProviderDef, ProactiveAcceleration};
use std::sync::Arc;

const LOCAL_EMBED_PROVIDER: &str = "embedding_local";

fn ensure_local_embedding_provider(ai: &mut AiConfig) {
    if !ai.providers.contains_key(LOCAL_EMBED_PROVIDER) {
        ai.providers.insert(
            LOCAL_EMBED_PROVIDER.to_string(),
            AiProviderDef::LocalGguf {
                model: "jina-embeddings-v5-text-small".to_string(),
                quantization: "F16".to_string(),
                model_path: String::new(),
                acceleration: ProactiveAcceleration::default(),
                gpu_layers: "auto".to_string(),
                context_size: 2048,
            },
        );
    }
    ai.tasks.embedding.provider = LOCAL_EMBED_PROVIDER.to_string();
    if ai.tasks.embedding.model.is_none() {
        ai.tasks.embedding.model = Some("jina-embeddings-v5-text-small".to_string());
    }
}

fn set_embedding_cloud(ai: &mut AiConfig) {
    ai.tasks.embedding.provider = "default".to_string();
    if ai.tasks.embedding.model.is_none() {
        ai.tasks.embedding.model = Some("text-embedding-3-small".to_string());
    }
    ai.tasks.embedding.dimensions = Some(1536);
}

pub fn render(
    ui: &mut egui::Ui,
    settings: &mut CharacterSettings,
    _animation: &mut AnimationControl,
    ai: &Arc<AiBridge>,
    input: &mut SettingsInputState,
    _world: &mut World,
    _ui_entity: Entity,
) {
    let mut ai_cfg = settings
        .ai
        .ai
        .get_section::<ene_runtime::AiConfig>()
        .unwrap_or_default();
    let mut memory = settings
        .ai
        .ai
        .get_section::<ene_store::StoreConfig>()
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

        ui.separator();
        ui.label("Chat");

        ui.horizontal(|ui| {
            ui.label(crate::i18n::model());
            let response = ui.add(
                egui::TextEdit::singleline(&mut input.ai_chat_model).desired_width(f32::INFINITY),
            );
            if response.changed() {
                ai_cfg.tasks.chat.model = Some(input.ai_chat_model.trim().to_string());
                let _ = settings.ai.ai.set_section(&ai_cfg);
                settings.mark_dirty();
            }
        });

        ui.horizontal(|ui| {
            ui.label(crate::i18n::base_url());
            let response = ui.add(
                egui::TextEdit::singleline(&mut input.ai_base_url).desired_width(f32::INFINITY),
            );
            if response.changed() {
                let url = input.ai_base_url.trim().to_string();
                let provider_key = ai_cfg.tasks.chat.provider.clone();
                match ai_cfg.providers.get_mut(&provider_key) {
                    Some(AiProviderDef::OpenaiCompatible { base_url, .. }) => {
                        *base_url = url;
                    }
                    _ => {
                        ai_cfg.providers.insert(
                            provider_key,
                            AiProviderDef::OpenaiCompatible {
                                base_url: url,
                                api_key: ene_ai::ApiKeyConfig::default(),
                            },
                        );
                    }
                }
                let _ = settings.ai.ai.set_section(&ai_cfg);
                settings.mark_dirty();
            }
        });

        ui.horizontal(|ui| {
            ui.label(crate::i18n::api_key_source());
            let mut current_source = input.ai_api_key_source.clone();
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
            if current_source != input.ai_api_key_source {
                input.ai_api_key_source.clone_from(&current_source);
                let provider_key = ai_cfg.tasks.chat.provider.clone();
                if let Some(AiProviderDef::OpenaiCompatible { api_key, .. }) =
                    ai_cfg.providers.get_mut(&provider_key)
                {
                    api_key.source = current_source;
                }
                let _ = settings.ai.ai.set_section(&ai_cfg);
                settings.mark_dirty();
            }
        });

        if input.ai_api_key_source == "env" {
            ui.horizontal(|ui| {
                ui.label(crate::i18n::api_key_env_var());
                let response = ui.add(
                    egui::TextEdit::singleline(&mut input.ai_api_key_env)
                        .desired_width(f32::INFINITY),
                );
                if response.changed() {
                    let provider_key = ai_cfg.tasks.chat.provider.clone();
                    if let Some(AiProviderDef::OpenaiCompatible { api_key, .. }) =
                        ai_cfg.providers.get_mut(&provider_key)
                    {
                        api_key.env = input.ai_api_key_env.trim().to_string();
                        let _ = settings.ai.ai.set_section(&ai_cfg);
                        settings.mark_dirty();
                    }
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
                    let provider_key = ai_cfg.tasks.chat.provider.clone();
                    if let Some(AiProviderDef::OpenaiCompatible { api_key, .. }) =
                        ai_cfg.providers.get_mut(&provider_key)
                    {
                        api_key.inline = input.ai_api_key.trim().to_string();
                        let _ = settings.ai.ai.set_section(&ai_cfg);
                        settings.mark_dirty();
                    }
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
                input.ai_embedding_provider.clone_from(&current_provider);
                if current_provider.as_str() == "local" {
                    ensure_local_embedding_provider(&mut ai_cfg);
                    input.ai_embedding_model.clone_from(
                        ai_cfg
                            .tasks
                            .embedding
                            .model
                            .as_ref()
                            .unwrap_or(&String::new()),
                    );
                    input.ai_embedding_dimensions = "auto".to_string();
                } else {
                    set_embedding_cloud(&mut ai_cfg);
                    input.ai_embedding_model.clone_from(
                        ai_cfg
                            .tasks
                            .embedding
                            .model
                            .as_ref()
                            .unwrap_or(&String::new()),
                    );
                    input.ai_embedding_dimensions = ai_cfg
                        .tasks
                        .embedding
                        .dimensions
                        .map_or_else(|| "1536".to_string(), |d| d.to_string());
                }
                let _ = settings.ai.ai.set_section(&ai_cfg);
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
                ai_cfg.tasks.embedding.model = Some(input.ai_embedding_model.trim().to_string());
                if input.ai_embedding_provider == "local"
                    && let Some(AiProviderDef::LocalGguf { model, .. }) =
                        ai_cfg.providers.get_mut(LOCAL_EMBED_PROVIDER)
                {
                    *model = input.ai_embedding_model.trim().to_string();
                }
                let _ = settings.ai.ai.set_section(&ai_cfg);
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
                    ai_cfg.tasks.embedding.dimensions = Some(dims);
                    let _ = settings.ai.ai.set_section(&ai_cfg);
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
        ui.label(crate::i18n::proactive_speech());

        let mut mind = settings
            .ai
            .ai
            .get_section::<ene_mind::MindConfig>()
            .unwrap_or_default();

        ui.horizontal(|ui| {
            let mut enabled = mind.proactive.enabled;
            ui.checkbox(&mut enabled, crate::i18n::proactive_enabled());
            if enabled != mind.proactive.enabled {
                mind.proactive.enabled = enabled;
                let _ = settings.ai.ai.set_section(&mind);
                settings.mark_dirty();
                ai.sync_proactive_runtime(&mind);
            }
        });

        ui.horizontal(|ui| {
            ui.label(crate::i18n::proactive_interval());
            let mut value = mind.proactive.interval_seconds as i32;
            if ui
                .add(egui::DragValue::new(&mut value).range(1..=3600))
                .changed()
            {
                mind.proactive.interval_seconds = value.max(1) as u64;
                let _ = settings.ai.ai.set_section(&mind);
                settings.mark_dirty();
                ai.sync_proactive_runtime(&mind);
            }
        });

        ui.horizontal(|ui| {
            ui.label(crate::i18n::proactive_cooldown());
            let mut value = mind.proactive.cooldown_seconds as i32;
            if ui
                .add(egui::DragValue::new(&mut value).range(0..=86_400))
                .changed()
            {
                mind.proactive.cooldown_seconds = value.max(0) as u64;
                let _ = settings.ai.ai.set_section(&mind);
                settings.mark_dirty();
                ai.sync_proactive_runtime(&mind);
            }
        });

        ui.horizontal(|ui| {
            ui.label(crate::i18n::proactive_min_idle());
            let mut value = mind.proactive.min_idle_seconds as i32;
            if ui
                .add(egui::DragValue::new(&mut value).range(0..=86_400))
                .changed()
            {
                mind.proactive.min_idle_seconds = value.max(0) as u64;
                let _ = settings.ai.ai.set_section(&mind);
                settings.mark_dirty();
                ai.sync_proactive_runtime(&mind);
            }
        });
    });
}
