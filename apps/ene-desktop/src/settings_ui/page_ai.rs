//! AI settings page — chat provider and embedding configuration.
//!
//! Chat lives in the dedicated chat window (F2); see #109.
//! Feature toggles and proactive speech policy live on the Features tab.

use super::input::SettingsInputState;
use crate::ai_bridge::AiBridge;
use crate::character_state::AnimationControl;
use crate::settings::CharacterSettings;
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
use ene_ai::{AiConfig, AiProviderDef, LOCAL_PROVIDER, LocalModelDef};
use std::sync::Arc;

const DEFAULT_LOCAL_EMBED_MODEL: &str = "jina-v5-small";

fn first_openai_compatible_provider(ai: &AiConfig) -> Option<String> {
    ai.providers.iter().find_map(|(name, def)| {
        matches!(def, AiProviderDef::OpenaiCompatible { .. }).then(|| name.clone())
    })
}

fn ensure_local_embedding_provider(ai: &mut AiConfig) {
    if !ai.local_models.contains_key(DEFAULT_LOCAL_EMBED_MODEL) {
        ai.local_models.insert(
            DEFAULT_LOCAL_EMBED_MODEL.to_string(),
            LocalModelDef {
                url: "https://huggingface.co/jinaai/jina-embeddings-v5-text-small-retrieval/resolve/main/v5-small-retrieval-F16.gguf".to_string(),
                quantization: "F16".to_string(),
                ..LocalModelDef::default()
            },
        );
    }
    ai.tasks.embedding.provider = LOCAL_PROVIDER.to_string();
    if ai.tasks.embedding.model.is_none() {
        ai.tasks.embedding.model = Some(DEFAULT_LOCAL_EMBED_MODEL.to_string());
    }
}

fn set_embedding_cloud(ai: &mut AiConfig) {
    let chat_key = ai.tasks.chat.provider.clone();
    let provider_key = match ai.providers.get(&chat_key) {
        Some(AiProviderDef::OpenaiCompatible { .. }) => chat_key,
        _ => first_openai_compatible_provider(ai).unwrap_or_else(|| {
            ai.providers.insert(
                chat_key.clone(),
                AiProviderDef::OpenaiCompatible {
                    base_url: String::new(),
                    api_key: ene_ai::ApiKeyConfig::default(),
                },
            );
            chat_key
        }),
    };
    ai.tasks.embedding.provider = provider_key;
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
        render_provider_health(ui, ai, &ai_cfg);
    });
}

/// Provider health and failover status display (#175).
fn render_provider_health(ui: &mut egui::Ui, ai: &Arc<AiBridge>, ai_cfg: &AiConfig) {
    ui.label(crate::i18n::provider_health());

    if !ai_cfg.fallback.enabled {
        ui.weak(crate::i18n::provider_health_failover_disabled());
        return;
    }

    let reports = ai.provider_health_reports();
    if reports.is_empty() {
        ui.weak(crate::i18n::provider_health_no_reports());
        return;
    }

    egui::Grid::new("provider_health_grid")
        .striped(true)
        .show(ui, |ui| {
            ui.strong(crate::i18n::provider());
            ui.strong(crate::i18n::provider_health_status());
            ui.strong(crate::i18n::provider_health_latency());
            ui.strong(crate::i18n::provider_health_detail());
            ui.end_row();
            for report in &reports {
                ui.label(&report.provider);
                let status = report.status.status_code();
                let color = if report.status.is_available() {
                    egui::Color32::from_rgb(120, 200, 120)
                } else {
                    egui::Color32::from_rgb(220, 110, 110)
                };
                ui.colored_label(color, status);
                ui.label(format!("{}ms", report.latency_ms));
                ui.label(report.error.clone().unwrap_or_default());
                ui.end_row();
            }
        });

    let history = ai.provider_fallback_history();
    ui.add_space(8.0);
    ui.label(crate::i18n::provider_health_fallback_history());
    if history.is_empty() {
        ui.weak(crate::i18n::provider_health_no_fallbacks());
    } else {
        for record in history.iter().rev().take(5) {
            ui.weak(format!(
                "{} → {} ({})",
                record.from, record.to, record.reason
            ));
        }
    }
}
