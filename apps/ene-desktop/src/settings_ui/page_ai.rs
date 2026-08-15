//! AI settings page — chat provider and embedding configuration.
//!
//! Chat lives in the dedicated chat window (F2); feature toggles
//! and proactive speech policy live on the Features tab.

use super::components::{BadgeTone, section_card, setting_row, status_badge, warning_box};
use super::draft::{SecretState, SettingsDraft};
use super::input::{AsyncData, SettingsInputState};
use super::widgets::editable_combo;
use crate::ai_bridge::AiBridge;
use crate::character_state::AnimationControl;
use crate::settings::CharacterSettings;
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
use ene_ai::{AiConfig, AiProviderDef, LOCAL_PROVIDER};
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

/// Records the secret state of the chat provider's inline API key so the
/// draft never echoes the stored value into the UI and never overwrites an
/// unchanged secret with an empty buffer.
fn track_api_key_secret(draft: &mut SettingsDraft, provider: &str, source: &str, buffer: &str) {
    let path = format!("ai.providers.{provider}.api_key.inline");
    if source == "env" {
        draft.set_secret(&path, SecretState::EnvSource);
    } else if buffer.is_empty() {
        // A user-initiated deletion wins over the "unchanged" default; the
        // delete button above marks `Deleted` explicitly.
        if draft.secret(&path) != SecretState::Deleted {
            draft.set_secret(&path, SecretState::Unchanged);
        }
    } else {
        draft.set_secret(&path, SecretState::Replaced);
    }
}
/// Static embedding-model fallback for cloud providers (same fallback
/// rationale as [`CHAT_MODEL_FALLBACK`]).
const EMBEDDING_MODEL_FALLBACK: &[&str] = &[
    "text-embedding-3-small",
    "text-embedding-3-large",
    "text-embedding-ada-002",
];
const EMBEDDING_DIMENSION_CHOICES: &[&str] = &["512", "768", "1024", "1536", "3072"];

fn ensure_local_embedding_provider(ai: &mut AiConfig, draft: &mut SettingsDraft, engine: &str) {
    // Plugin profiles are the single source of truth for local models:
    // `ai.local_models` is derived from them at apply time.
    let mut plugins = draft.section::<ene_plugin_host::PluginConfig>();
    let entry = plugins.list.entry(engine.to_string()).or_default();
    entry.enable = true;
    if !entry.profiles.contains_key(DEFAULT_LOCAL_EMBED_MODEL) {
        entry.profiles.insert(
            DEFAULT_LOCAL_EMBED_MODEL.to_string(),
            serde_json::json!({
                "url": "https://huggingface.co/jinaai/jina-embeddings-v5-text-small-retrieval/resolve/main/v5-small-retrieval-F16.gguf",
                "quantization": "F16",
                "dimensions": 1024
            }),
        );
    }
    draft.set_section(&plugins);
    ai.tasks.embedding.provider = LOCAL_PROVIDER.to_string();
    ai.local_engine = engine.to_string();
    if ai.tasks.embedding.model.is_none() {
        ai.tasks.embedding.model = Some(DEFAULT_LOCAL_EMBED_MODEL.to_string());
    }
}

/// Applies a typed cloud provider key to the embedding task. The key is
/// registered when it was not in `ai.providers` yet (same behaviour as the
/// chat row), and model / dimensions get their defaults only on first use
/// so an existing cloud selection keeps its values.
fn select_embedding_cloud(ai: &mut AiConfig, provider: &str) {
    ai.tasks.embedding.provider = provider.to_string();
    if !ai.providers.contains_key(provider) {
        ai.providers
            .insert(provider.to_string(), AiProviderDef::default());
    }
    if ai.tasks.embedding.model.is_none() {
        ai.tasks.embedding.model = Some("text-embedding-3-small".to_string());
    }
    if ai.tasks.embedding.dimensions.is_none() {
        ai.tasks.embedding.dimensions = Some(1536);
    }
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

/// Provider choices shared by the chat and embedding rows: the concrete
/// local engines first, then every registered provider definition.
fn provider_picker_choices(ai: &AiConfig) -> Vec<(String, String)> {
    let mut choices = ene_ai::LOCAL_ENGINE_CHOICES
        .iter()
        .map(|engine| ((*engine).to_string(), (*engine).to_string()))
        .collect::<Vec<_>>();
    choices.extend(provider_choices(ai));
    choices
}

/// Whether a picker buffer value names one of the local engines (as opposed
/// to a cloud provider key).
fn is_local_engine(value: &str) -> bool {
    ene_ai::LOCAL_ENGINE_CHOICES.contains(&value)
}

fn local_profile_names(draft: &SettingsDraft) -> Vec<String> {
    let plugins = draft.section::<ene_plugin_host::PluginConfig>();
    let mut names: Vec<String> = ["llama-cpp", "local-llm", "llama-server"]
        .iter()
        .filter_map(|plugin| plugins.list.get(*plugin))
        .flat_map(|entry| entry.profiles.keys().cloned())
        .collect();
    names.sort();
    names.dedup();
    names
}

fn chat_model_choices(
    ai: &AiConfig,
    input: &SettingsInputState,
    local_profiles: &[String],
) -> Vec<(String, String)> {
    let mut choices = Vec::new();
    if ai.tasks.chat.provider == LOCAL_PROVIDER {
        choices.extend(
            local_profiles
                .iter()
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
fn embedding_model_choices(
    ai: &AiConfig,
    input: &SettingsInputState,
    local_profiles: &[String],
) -> Vec<(String, String)> {
    let mut choices = Vec::new();
    if AiConfig::is_local_provider(&ai.tasks.embedding.provider) {
        choices.extend(
            local_profiles
                .iter()
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

fn sync_provider_buffers(input: &mut SettingsInputState, ai: &AiConfig) {
    if let Some(def) = ai.providers.get(&ai.tasks.chat.provider) {
        input.ai_base_url.clone_from(&def.base_url);
        input.ai_api_key_source.clone_from(&def.api_key.source);
        // Secrets never round-trip into UI text buffers, including on
        // provider switches: the draft tracks them by state instead.
        input.ai_api_key.clear();
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
    draft: &mut SettingsDraft,
    _animation: &mut AnimationControl,
    ai: &Arc<AiBridge>,
    input: &mut SettingsInputState,
    _world: &mut World,
    _ui_entity: Entity,
) {
    let mut ai_cfg = draft.section::<ene_runtime::AiConfig>();
    // Local-model resolution reads `ai.local_models`, which is derived from
    // plugin profiles; mirror the derivation into the working copy so
    // choices, resolution, and test-connection agree with what apply will
    // persist.
    ai_cfg.local_models = super::apply::local_model_defs_from_plugins(draft.editing());
    // Resolution (test connection, model list) must use the *merged*
    // config: the draft holds redacted secrets, so resolving against it
    // would send the placeholder as the API key.
    let mut resolved_ai_cfg = super::apply::merge_secrets(&settings.config(), draft.editing())
        .get_section::<ene_runtime::AiConfig>()
        .unwrap_or_else(|_| ai_cfg.clone());
    resolved_ai_cfg.local_models = ai_cfg.local_models.clone();

    input.model_list.poll();
    if let Some(models) = input.model_list.data.take() {
        input
            .model_catalog
            .insert(ai_cfg.tasks.chat.provider.clone(), models);
    }
    input.api_test.poll();
    if let Some(result) = input.api_test.data.take() {
        input.ai_validation_message = Some(match result {
            Ok(()) => {
                i18n_embed_fl::fl!(crate::i18n::loader(), "ai-test-connection-ok").to_string()
            }
            Err(error) => format!(
                "{}: {error}",
                i18n_embed_fl::fl!(crate::i18n::loader(), "ai-test-connection-error")
            ),
        });
    }

    ui.vertical(|ui| {
        let local_profiles = local_profile_names(draft);
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
                            draft.set_path(
                                "user_name",
                                serde_json::Value::String(input.ai_user_name.trim().to_string()),
                            );
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
                        let mut choices = provider_picker_choices(&ai_cfg);
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
                                if is_local_engine(&new_provider) {
                                    ai_cfg.tasks.chat.provider = LOCAL_PROVIDER.to_string();
                                    ai_cfg.local_engine.clone_from(&new_provider);
                                    let mut plugins =
                                        draft.section::<ene_plugin_host::PluginConfig>();
                                    plugins.list.entry(new_provider.clone()).or_default().enable =
                                        true;
                                    draft.set_section(&plugins);
                                } else {
                                    ai_cfg.tasks.chat.provider.clone_from(&new_provider);
                                    if !ai_cfg.providers.contains_key(&new_provider) {
                                        ai_cfg
                                            .providers
                                            .insert(new_provider.clone(), AiProviderDef::default());
                                    }
                                }
                                sync_provider_buffers(input, &ai_cfg);
                                draft.set_section(&ai_cfg);
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
                        let choices = chat_model_choices(&ai_cfg, input, &local_profiles);
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
                            input.model_list = AsyncData::new();
                            if let Ok(resolved) = resolved_ai_cfg.resolve_chat() {
                                input.model_list.start(ai.apply_fetch_model_ids(
                                    resolved.base_url.clone(),
                                    resolved.api_key.clone(),
                                ));
                            } else {
                                input.model_fetch_error = Some(
                                    i18n_embed_fl::fl!(
                                        crate::i18n::loader(),
                                        "ai-test-connection-error"
                                    )
                                    .to_string(),
                                );
                            }
                        }
                        if combo.commit_requested() {
                            ai_cfg.tasks.chat.model = Some(input.ai_chat_model.trim().to_string());
                            draft.set_section(&ai_cfg);
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
                            draft.set_section(&ai_cfg);
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
                            draft.set_section(&ai_cfg);
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
                                    draft.set_section(&ai_cfg);
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
                            ui.horizontal(|ui| {
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
                                        draft.set_section(&ai_cfg);
                                    }
                                }
                                if ui
                                    .small_button(i18n_embed_fl::fl!(
                                        crate::i18n::loader(),
                                        "api-key-delete"
                                    ))
                                    .on_hover_text(i18n_embed_fl::fl!(
                                        crate::i18n::loader(),
                                        "api-key-delete-hint"
                                    ))
                                    .clicked()
                                {
                                    let provider_key = ai_cfg.tasks.chat.provider.clone();
                                    if let Some(def) = ai_cfg.providers.get_mut(&provider_key)
                                        && def.is_openai_compatible()
                                    {
                                        def.api_key.inline.clear();
                                        draft.set_section(&ai_cfg);
                                        draft.set_secret(
                                            &format!("ai.providers.{provider_key}.api_key.inline"),
                                            SecretState::Deleted,
                                        );
                                        input.ai_api_key.clear();
                                    }
                                }
                            });
                        },
                    );
                }
                track_api_key_secret(
                    draft,
                    ai_cfg.tasks.chat.provider.as_str(),
                    &input.ai_api_key_source,
                    &input.ai_api_key,
                );

                let issues = ene_ai::validate_settings(draft.editing());
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
                    input.ai_validation_message = None;
                    input.api_test = AsyncData::new();
                    if let Ok(resolved) = resolved_ai_cfg.resolve_chat() {
                        input.api_test.start(ai.apply_validate_api_key(
                            resolved.base_url.clone(),
                            resolved.api_key.clone(),
                        ));
                    } else {
                        input.ai_validation_message = Some(
                            i18n_embed_fl::fl!(crate::i18n::loader(), "ai-test-connection-error")
                                .to_string(),
                        );
                    }
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
                        let mut choices = provider_picker_choices(&ai_cfg);
                        if !choices
                            .iter()
                            .any(|(value, _)| value == &input.ai_embedding_provider)
                        {
                            choices.insert(
                                0,
                                (
                                    input.ai_embedding_provider.clone(),
                                    input.ai_embedding_provider.clone(),
                                ),
                            );
                        }
                        let combo = editable_combo(
                            ui,
                            "embedding_provider_combo",
                            &mut input.ai_embedding_provider,
                            &choices,
                            200.0,
                        );
                        if combo.commit_requested() {
                            let new_provider = input.ai_embedding_provider.trim().to_string();
                            if !new_provider.is_empty()
                                && new_provider != ai_cfg.tasks.embedding.provider
                            {
                                if is_local_engine(&new_provider) {
                                    ensure_local_embedding_provider(
                                        &mut ai_cfg,
                                        draft,
                                        &new_provider,
                                    );
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
                                    select_embedding_cloud(&mut ai_cfg, &new_provider);
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
                                draft.set_section(&ai_cfg);
                            }
                        }
                    },
                );

                setting_row(
                    ui,
                    "embedding_model_row",
                    &i18n_embed_fl::fl!(crate::i18n::loader(), "model"),
                    "",
                    |ui| {
                        let choices = embedding_model_choices(&ai_cfg, input, &local_profiles);
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
                            draft.set_section(&ai_cfg);
                        }
                    },
                );

                setting_row(
                    ui,
                    "embedding_dimensions_row",
                    &i18n_embed_fl::fl!(crate::i18n::loader(), "dimensions"),
                    "",
                    |ui| {
                        if is_local_engine(&input.ai_embedding_provider) {
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
                                draft.set_section(&ai_cfg);
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
