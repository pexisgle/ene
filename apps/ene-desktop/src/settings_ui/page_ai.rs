//! AI settings page — chat provider and embedding configuration.
//!
//! Chat lives in the dedicated chat window (F2); feature toggles
//! and proactive speech policy live on the Features tab.

use super::components::{BadgeTone, section_card, setting_row, status_badge, warning_box};
use super::input::SettingsInputState;
use super::widgets::editable_combo;
use crate::ai_bridge::AiBridge;
use crate::character_state::AnimationControl;
use crate::settings::CharacterSettings;
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
use ene_ai::{AiConfig, AiProviderDef, LOCAL_PROVIDER, LocalModelDef};
use std::sync::Arc;

const DEFAULT_LOCAL_EMBED_MODEL: &str = "jina-v5-small";
/// Static chat-model fallback when the provider's `/models` endpoint is
/// unreachable or not configured yet. The free-form editor always accepts
/// any other model name.
const CHAT_MODEL_FALLBACK: &[&str] = &[
    "gpt-4o",
    "gpt-4o-mini",
    "gpt-4.1",
    "gpt-4.1-mini",
    "gpt-5",
    "gpt-5-mini",
];
/// Static embedding-model fallback for cloud providers (same fallback
/// rationale as [`CHAT_MODEL_FALLBACK`]).
const EMBEDDING_MODEL_FALLBACK: &[&str] = &[
    "text-embedding-3-small",
    "text-embedding-3-large",
    "text-embedding-ada-002",
];
/// Common embedding vector sizes offered as one-click choices.
const EMBEDDING_DIMENSION_CHOICES: &[&str] = &["512", "768", "1024", "1536", "3072"];

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
                dimensions: Some(1024),
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

/// Provider keys as (value, label) choices, OpenAI-compatible entries first.
fn provider_choices(ai: &AiConfig) -> Vec<(String, String)> {
    let mut compatible = Vec::new();
    let mut others = Vec::new();
    for (name, def) in &ai.providers {
        let label = if def.is_openai_compatible() {
            name.clone()
        } else {
            format!("{name} ({})", def.kind)
        };
        if def.is_openai_compatible() {
            compatible.push((name.clone(), label));
        } else {
            others.push((name.clone(), label));
        }
    }
    compatible.append(&mut others);
    compatible
}

/// Chat model candidates for the current chat provider.
///
/// Local providers list registered GGUF models; cloud providers use the
/// cached `/models` catalog (when fetched) plus the static fallback. The
/// currently configured model is always included so an out-of-catalog value
/// stays selectable and visible.
fn chat_model_choices(ai: &AiConfig, input: &SettingsInputState) -> Vec<(String, String)> {
    let mut choices = Vec::new();
    if ai.tasks.chat.provider == LOCAL_PROVIDER {
        choices.extend(
            ai.local_models
                .keys()
                .map(|name| (name.clone(), name.clone())),
        );
    } else {
        if let Some(models) = input.model_catalog.get(&ai.tasks.chat.provider) {
            choices.extend(models.iter().map(|m| (m.clone(), m.clone())));
        }
        choices.extend(
            CHAT_MODEL_FALLBACK
                .iter()
                .map(|m| ((*m).to_string(), (*m).to_string())),
        );
    }
    if let Some(current) = ai.tasks.chat.model.as_deref()
        && !choices.iter().any(|(value, _)| value == current)
    {
        choices.insert(0, (current.to_string(), current.to_string()));
    }
    choices
}

/// Embedding model candidates for the current embedding provider.
fn embedding_model_choices(ai: &AiConfig, input: &SettingsInputState) -> Vec<(String, String)> {
    let mut choices = Vec::new();
    if AiConfig::is_local_provider(&ai.tasks.embedding.provider) {
        choices.extend(
            ai.local_models
                .keys()
                .map(|name| (name.clone(), name.clone())),
        );
    } else {
        if let Some(models) = input.model_catalog.get(&ai.tasks.embedding.provider) {
            choices.extend(models.iter().map(|m| (m.clone(), m.clone())));
        }
        choices.extend(
            EMBEDDING_MODEL_FALLBACK
                .iter()
                .map(|m| ((*m).to_string(), (*m).to_string())),
        );
    }
    if let Some(current) = ai.tasks.embedding.model.as_deref()
        && !choices.iter().any(|(value, _)| value == current)
    {
        choices.insert(0, (current.to_string(), current.to_string()));
    }
    choices
}

/// Re-sync the editable provider buffers to the currently selected chat
/// provider's definition after a provider switch.
fn sync_provider_buffers(input: &mut SettingsInputState, ai: &AiConfig) {
    if let Some(def) = ai.providers.get(&ai.tasks.chat.provider) {
        input.ai_base_url.clone_from(&def.base_url);
        input.ai_api_key_source.clone_from(&def.api_key.source);
        input.ai_api_key.clone_from(&def.api_key.inline);
        input.ai_api_key_env.clone_from(&def.api_key.env);
    } else {
        input.ai_base_url.clear();
        input.ai_api_key_source = "env".to_string();
        input.ai_api_key.clear();
        input.ai_api_key_env = "OPENAI_API_KEY".to_string();
    }
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
        section_card(
            ui,
            "ai-chat",
            &i18n_embed_fl::fl!(crate::i18n::loader(), "section-ai-chat"),
            |ui| {
                ui.weak(i18n_embed_fl::fl!(crate::i18n::loader(), "chat-open-hint"));
                ui.add_space(4.0);
                setting_row(
                    ui,
                    "ai_character_card_row",
                    &i18n_embed_fl::fl!(crate::i18n::loader(), "character-card"),
                    "",
                    |ui| {
                        ui.label(settings.current_character_card().unwrap_or("—"));
                    },
                );
                setting_row(
                    ui,
                    "ai_user_name_row",
                    &i18n_embed_fl::fl!(crate::i18n::loader(), "user-name"),
                    "",
                    |ui| {
                        let response = ui.add(
                            egui::TextEdit::singleline(&mut input.ai_user_name)
                                .desired_width(260.0),
                        );
                        if response.changed() {
                            settings.with_config_mut(|c| {
                                c.user_name = input.ai_user_name.trim().to_string();
                            });
                            settings.mark_dirty();
                        }
                    },
                );

                ui.add_space(4.0);
                setting_row(
                    ui,
                    "chat_provider_row",
                    &i18n_embed_fl::fl!(crate::i18n::loader(), "provider"),
                    "",
                    |ui| {
                        let mut choices = provider_choices(&ai_cfg);
                        if !choices
                            .iter()
                            .any(|(value, _)| value == &input.ai_chat_provider)
                        {
                            choices.insert(
                                0,
                                (
                                    input.ai_chat_provider.clone(),
                                    input.ai_chat_provider.clone(),
                                ),
                            );
                        }
                        let combo = editable_combo(
                            ui,
                            "chat_provider_combo",
                            &mut input.ai_chat_provider,
                            &choices,
                            200.0,
                        );
                        if combo.commit_requested() {
                            let new_provider = input.ai_chat_provider.trim().to_string();
                            if !new_provider.is_empty()
                                && new_provider != ai_cfg.tasks.chat.provider
                            {
                                ai_cfg.tasks.chat.provider.clone_from(&new_provider);
                                if !ai_cfg.providers.contains_key(&new_provider) {
                                    ai_cfg
                                        .providers
                                        .insert(new_provider.clone(), AiProviderDef::default());
                                }
                                sync_provider_buffers(input, &ai_cfg);
                                settings.set_config_section(&ai_cfg);
                                settings.mark_dirty();
                            }
                        }
                    },
                );

                setting_row(
                    ui,
                    "chat_model_row",
                    &i18n_embed_fl::fl!(crate::i18n::loader(), "model"),
                    "",
                    |ui| {
                        let choices = chat_model_choices(&ai_cfg, input);
                        let combo = editable_combo(
                            ui,
                            "chat_model_combo",
                            &mut input.ai_chat_model,
                            &choices,
                            220.0,
                        );
                        if ui
                            .button(i18n_embed_fl::fl!(
                                crate::i18n::loader(),
                                "model-list-refresh"
                            ))
                            .on_hover_text(i18n_embed_fl::fl!(
                                crate::i18n::loader(),
                                "model-list-refresh-hint"
                            ))
                            .clicked()
                        {
                            input.model_fetch_error = None;
                            match ai_cfg.resolve_chat() {
                                Ok(resolved) => {
                                    match ai
                                        .list_models_blocking(&resolved.base_url, &resolved.api_key)
                                    {
                                        Ok(models) => {
                                            input
                                                .model_catalog
                                                .insert(ai_cfg.tasks.chat.provider.clone(), models);
                                        }
                                        Err(e) => input.model_fetch_error = Some(e.to_string()),
                                    }
                                }
                                Err(e) => input.model_fetch_error = Some(e.to_string()),
                            }
                        }
                        if combo.commit_requested() {
                            ai_cfg.tasks.chat.model = Some(input.ai_chat_model.trim().to_string());
                            settings.set_config_section(&ai_cfg);
                            settings.mark_dirty();
                        }
                    },
                );
                if let Some(error) = input.model_fetch_error.as_deref() {
                    ui.weak(error);
                }

                setting_row(
                    ui,
                    "base_url_row",
                    &i18n_embed_fl::fl!(crate::i18n::loader(), "base-url"),
                    "",
                    |ui| {
                        let response = ui.add(
                            egui::TextEdit::singleline(&mut input.ai_base_url).desired_width(260.0),
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
                    },
                );

                setting_row(
                    ui,
                    "api_key_source_row",
                    &i18n_embed_fl::fl!(crate::i18n::loader(), "api-key-source"),
                    "",
                    |ui| {
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
                    },
                );

                if input.ai_api_key_source == "env" {
                    setting_row(
                        ui,
                        "api_key_env_row",
                        &i18n_embed_fl::fl!(crate::i18n::loader(), "api-key-env-var"),
                        "",
                        |ui| {
                            let response = ui.add(
                                egui::TextEdit::singleline(&mut input.ai_api_key_env)
                                    .desired_width(260.0),
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
                        },
                    );
                } else {
                    setting_row(
                        ui,
                        "api_key_row",
                        &i18n_embed_fl::fl!(crate::i18n::loader(), "api-key"),
                        "",
                        |ui| {
                            let response = ui.add(
                                egui::TextEdit::singleline(&mut input.ai_api_key)
                                    .password(true)
                                    .desired_width(260.0),
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
                        },
                    );
                }

                let issues = ene_ai::validate_settings(&settings.config());
                for issue in &issues {
                    warning_box(ui, &issue.message());
                }
                if ui
                    .button(i18n_embed_fl::fl!(
                        crate::i18n::loader(),
                        "ai-test-connection"
                    ))
                    .clicked()
                {
                    input.ai_validation_message = Some(match ai_cfg.resolve_chat() {
                        Ok(resolved) => {
                            match ai
                                .validate_api_key_blocking(&resolved.base_url, &resolved.api_key)
                            {
                                Ok(()) => {
                                    i18n_embed_fl::fl!(
                                        crate::i18n::loader(),
                                        "ai-test-connection-ok"
                                    )
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
                if let Some(message) = input.ai_validation_message.as_deref() {
                    ui.label(message);
                }
            },
        );

        section_card(
            ui,
            "ai-embedding",
            &i18n_embed_fl::fl!(crate::i18n::loader(), "section-ai-embedding"),
            |ui| {
                setting_row(
                    ui,
                    "embedding_provider_row",
                    &i18n_embed_fl::fl!(crate::i18n::loader(), "provider"),
                    "",
                    |ui| {
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
                    },
                );

                setting_row(
                    ui,
                    "embedding_model_row",
                    &i18n_embed_fl::fl!(crate::i18n::loader(), "model"),
                    "",
                    |ui| {
                        let choices = embedding_model_choices(&ai_cfg, input);
                        let combo = editable_combo(
                            ui,
                            "embedding_model_combo",
                            &mut input.ai_embedding_model,
                            &choices,
                            220.0,
                        );
                        if combo.commit_requested() {
                            ai_cfg.tasks.embedding.model =
                                Some(input.ai_embedding_model.trim().to_string());
                            settings.set_config_section(&ai_cfg);
                            settings.mark_dirty();
                        }
                    },
                );

                setting_row(
                    ui,
                    "embedding_dimensions_row",
                    &i18n_embed_fl::fl!(crate::i18n::loader(), "dimensions"),
                    "",
                    |ui| {
                        if input.ai_embedding_provider == "local" {
                            ui.add_sized(
                                [100.0, 0.0],
                                egui::Label::new(i18n_embed_fl::fl!(
                                    crate::i18n::loader(),
                                    "auto-from-model"
                                )),
                            );
                        } else {
                            let choices = EMBEDDING_DIMENSION_CHOICES
                                .iter()
                                .map(|d| ((*d).to_string(), (*d).to_string()))
                                .collect::<Vec<_>>();
                            let combo = editable_combo(
                                ui,
                                "embedding_dimensions_combo",
                                &mut input.ai_embedding_dimensions,
                                &choices,
                                100.0,
                            );
                            if combo.commit_requested()
                                && let Ok(dims) = input.ai_embedding_dimensions.parse::<usize>()
                            {
                                ai_cfg.tasks.embedding.dimensions = Some(dims);
                                settings.set_config_section(&ai_cfg);
                                settings.mark_dirty();
                            }
                        }
                    },
                );
            },
        );

        section_card(
            ui,
            "ai-health",
            &i18n_embed_fl::fl!(crate::i18n::loader(), "section-ai-health"),
            |ui| render_provider_health(ui, ai, &ai_cfg),
        );
    });
}

fn render_provider_health(ui: &mut egui::Ui, ai: &Arc<AiBridge>, ai_cfg: &AiConfig) {
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

    if ui.available_width() < 600.0 {
        for report in &reports {
            egui::Frame::group(ui.style())
                .corner_radius(egui::CornerRadius::same(6))
                .inner_margin(egui::Margin::same(8))
                .show(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.strong(&report.provider);
                        status_badge(
                            ui,
                            report.status.status_code(),
                            if report.status.is_available() {
                                BadgeTone::Ok
                            } else {
                                BadgeTone::Error
                            },
                        );
                        ui.label(format!("{}ms", report.latency_ms));
                    });
                    if let Some(error) = report.error.as_deref() {
                        ui.weak(error);
                    }
                });
            ui.add_space(4.0);
        }
    } else {
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
                    status_badge(
                        ui,
                        status,
                        if report.status.is_available() {
                            BadgeTone::Ok
                        } else {
                            BadgeTone::Error
                        },
                    );
                    ui.label(format!("{}ms", report.latency_ms));
                    ui.label(report.error.clone().unwrap_or_default());
                    ui.end_row();
                }
            });
    }

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
