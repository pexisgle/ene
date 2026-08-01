//! AI settings page — chat provider and embedding configuration.
//!
//! Chat lives in the dedicated chat window (F2); feature toggles
//! and proactive speech policy live on the Features tab.

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
    ai.providers
        .iter()
        .find_map(|(name, def)| def.is_openai_compatible().then(|| name.clone()))
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
        Some(def) if def.is_openai_compatible() => chat_key,
        _ => first_openai_compatible_provider(ai).unwrap_or_else(|| {
            ai.providers
                .insert(chat_key.clone(), AiProviderDef::default());
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
    let mut ai_cfg = settings.config_section::<ene_runtime::AiConfig>();

    ui.vertical(|ui| {
        ui.weak(i18n_embed_fl::fl!(crate::i18n::loader(), "chat-open-hint"));
        ui.separator();

        ui.horizontal(|ui| {
            ui.label(i18n_embed_fl::fl!(crate::i18n::loader(), "character-card"));
            ui.add_sized(
                [220.0, 0.0],
                egui::Label::new(settings.current_character_card()),
            );
        });

        ui.horizontal(|ui| {
            ui.label(i18n_embed_fl::fl!(crate::i18n::loader(), "user-name"));
            let response = ui.add(
                egui::TextEdit::singleline(&mut input.ai_user_name).desired_width(f32::INFINITY),
            );
            if response.changed() {
                settings.with_config_mut(|c| c.user_name = input.ai_user_name.trim().to_string());
                settings.mark_dirty();
            }
        });

        ui.separator();
        ui.label("Chat");

        ui.horizontal(|ui| {
            ui.label(i18n_embed_fl::fl!(crate::i18n::loader(), "model"));
            let response = ui.add(
                egui::TextEdit::singleline(&mut input.ai_chat_model).desired_width(f32::INFINITY),
            );
            if response.changed() {
                ai_cfg.tasks.chat.model = Some(input.ai_chat_model.trim().to_string());
                settings.set_config_section(&ai_cfg);
                settings.mark_dirty();
            }
        });

        ui.horizontal(|ui| {
            ui.label(i18n_embed_fl::fl!(crate::i18n::loader(), "base-url"));
            let response = ui.add(
                egui::TextEdit::singleline(&mut input.ai_base_url).desired_width(f32::INFINITY),
            );
            if response.changed() {
                let url = input.ai_base_url.trim().to_string();
                let provider_key = ai_cfg.tasks.chat.provider.clone();
                match ai_cfg.providers.get_mut(&provider_key) {
                    Some(def) if def.is_openai_compatible() => {
                        def.base_url = url;
                    }
                    _ => {
                        ai_cfg.providers.insert(
                            provider_key,
                            AiProviderDef {
                                base_url: url,
                                ..AiProviderDef::default()
                            },
                        );
                    }
                }
                settings.set_config_section(&ai_cfg);
                settings.mark_dirty();
            }
        });

        ui.horizontal(|ui| {
            ui.label(i18n_embed_fl::fl!(crate::i18n::loader(), "api-key-source"));
            let mut current_source = input.ai_api_key_source.clone();
            egui::ComboBox::from_id_salt("api_key_source")
                .selected_text(current_source.as_str())
                .show_ui(ui, |ui| {
                    ui.selectable_value(
                        &mut current_source,
                        "inline".to_string(),
                        i18n_embed_fl::fl!(crate::i18n::loader(), "inline-settings"),
                    );
                    ui.selectable_value(
                        &mut current_source,
                        "env".to_string(),
                        i18n_embed_fl::fl!(crate::i18n::loader(), "environment"),
                    );
                });
            if current_source != input.ai_api_key_source {
                input.ai_api_key_source.clone_from(&current_source);
                let provider_key = ai_cfg.tasks.chat.provider.clone();
                if let Some(def) = ai_cfg.providers.get_mut(&provider_key)
                    && def.is_openai_compatible()
                {
                    def.api_key.source = current_source;
                }
                settings.set_config_section(&ai_cfg);
                settings.mark_dirty();
            }
        });

        if input.ai_api_key_source == "env" {
            ui.horizontal(|ui| {
                ui.label(i18n_embed_fl::fl!(crate::i18n::loader(), "api-key-env-var"));
                let response = ui.add(
                    egui::TextEdit::singleline(&mut input.ai_api_key_env)
                        .desired_width(f32::INFINITY),
                );
                if response.changed() {
                    let provider_key = ai_cfg.tasks.chat.provider.clone();
                    if let Some(def) = ai_cfg.providers.get_mut(&provider_key)
                        && def.is_openai_compatible()
                    {
                        def.api_key.env = input.ai_api_key_env.trim().to_string();
                        settings.set_config_section(&ai_cfg);
                        settings.mark_dirty();
                    }
                }
            });
        } else {
            ui.horizontal(|ui| {
                ui.label(i18n_embed_fl::fl!(crate::i18n::loader(), "api-key"));
                let response = ui.add(
                    egui::TextEdit::singleline(&mut input.ai_api_key)
                        .password(true)
                        .desired_width(f32::INFINITY),
                );
                if response.changed() {
                    let provider_key = ai_cfg.tasks.chat.provider.clone();
                    if let Some(def) = ai_cfg.providers.get_mut(&provider_key)
                        && def.is_openai_compatible()
                    {
                        def.api_key.inline = input.ai_api_key.trim().to_string();
                        settings.set_config_section(&ai_cfg);
                        settings.mark_dirty();
                    }
                }
            });
        }

        let issues = ene_ai::validate_settings(&settings.config());
        if !issues.is_empty() {
            for issue in &issues {
                ui.colored_label(egui::Color32::from_rgb(200, 120, 40), issue.message());
            }
        }
        ui.horizontal(|ui| {
            if ui
                .button(i18n_embed_fl::fl!(
                    crate::i18n::loader(),
                    "ai-test-connection"
                ))
                .clicked()
            {
                input.ai_validation_message = Some(match ai_cfg.resolve_chat() {
                    Ok(resolved) => {
                        match ai.validate_api_key_blocking(&resolved.base_url, &resolved.api_key) {
                            Ok(()) => {
                                i18n_embed_fl::fl!(crate::i18n::loader(), "ai-test-connection-ok")
                            }
                            Err(e) => format!(
                                "{}: {e}",
                                i18n_embed_fl::fl!(
                                    crate::i18n::loader(),
                                    "ai-test-connection-error"
                                )
                            ),
                        }
                    }
                    Err(e) => format!(
                        "{}: {e}",
                        i18n_embed_fl::fl!(crate::i18n::loader(), "ai-test-connection-error")
                    ),
                });
            }
        });
        if let Some(message) = input.ai_validation_message.as_deref() {
            ui.label(message);
        }

        ui.separator();
        ui.label(i18n_embed_fl::fl!(
            crate::i18n::loader(),
            "embedding-settings"
        ));

        ui.horizontal(|ui| {
            ui.label(i18n_embed_fl::fl!(crate::i18n::loader(), "provider"));
            let mut current_provider = input.ai_embedding_provider.clone();
            egui::ComboBox::from_id_salt("embedding_provider")
                .selected_text(current_provider.as_str())
                .show_ui(ui, |ui| {
                    ui.selectable_value(
                        &mut current_provider,
                        "cloud".to_string(),
                        i18n_embed_fl::fl!(crate::i18n::loader(), "cloud-api"),
                    );
                    ui.selectable_value(
                        &mut current_provider,
                        "local".to_string(),
                        i18n_embed_fl::fl!(crate::i18n::loader(), "local-gguf"),
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
                settings.set_config_section(&ai_cfg);
                settings.mark_dirty();
            }
        });

        ui.horizontal(|ui| {
            ui.label(i18n_embed_fl::fl!(crate::i18n::loader(), "model"));
            let response = ui.add(
                egui::TextEdit::singleline(&mut input.ai_embedding_model)
                    .desired_width(f32::INFINITY),
            );
            if response.changed() {
                ai_cfg.tasks.embedding.model = Some(input.ai_embedding_model.trim().to_string());
                settings.set_config_section(&ai_cfg);
                settings.mark_dirty();
            }
        });

        ui.horizontal(|ui| {
            ui.label(i18n_embed_fl::fl!(crate::i18n::loader(), "dimensions"));
            if input.ai_embedding_provider == "local" {
                ui.add_sized(
                    [100.0, 0.0],
                    egui::Label::new(i18n_embed_fl::fl!(crate::i18n::loader(), "auto-from-model")),
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
                    settings.set_config_section(&ai_cfg);
                    settings.mark_dirty();
                }
            }
        });

        ui.separator();
        render_provider_health(ui, ai, &ai_cfg);
    });
}

fn render_provider_health(ui: &mut egui::Ui, ai: &Arc<AiBridge>, ai_cfg: &AiConfig) {
    ui.label(i18n_embed_fl::fl!(crate::i18n::loader(), "provider-health"));

    if !ai_cfg.fallback.enabled {
        ui.weak(i18n_embed_fl::fl!(
            crate::i18n::loader(),
            "provider-health-failover-disabled"
        ));
        return;
    }

    let reports = ai.provider_health_reports();
    if reports.is_empty() {
        ui.weak(i18n_embed_fl::fl!(
            crate::i18n::loader(),
            "provider-health-no-reports"
        ));
        return;
    }

    egui::Grid::new("provider_health_grid")
        .striped(true)
        .show(ui, |ui| {
            ui.strong(i18n_embed_fl::fl!(crate::i18n::loader(), "provider"));
            ui.strong(i18n_embed_fl::fl!(
                crate::i18n::loader(),
                "provider-health-status"
            ));
            ui.strong(i18n_embed_fl::fl!(
                crate::i18n::loader(),
                "provider-health-latency"
            ));
            ui.strong(i18n_embed_fl::fl!(
                crate::i18n::loader(),
                "provider-health-detail"
            ));
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
    ui.label(i18n_embed_fl::fl!(
        crate::i18n::loader(),
        "provider-health-fallback-history"
    ));
    if history.is_empty() {
        ui.weak(i18n_embed_fl::fl!(
            crate::i18n::loader(),
            "provider-health-no-fallbacks"
        ));
    } else {
        for record in history.iter().rev().take(5) {
            ui.weak(format!(
                "{} → {} ({})",
                record.from, record.to, record.reason
            ));
        }
    }
}
