//! Voice settings page — Text-to-Speech (TTS), Speech-to-Text (STT), and
//! Microphone/VAD configuration.
//!
//! The page edits only the routing fields (`ai.tts.provider`,
//! `ai.stt.provider`). Every provider-owned value lives in
//! `plugins.list.<plugin>.config` and is rendered from the plugin's own JSON
//! Schema via [`provider_form`], so switching providers never mixes fields
//! from another provider and plugin schemas stay authoritative.

use super::components::{section_card, setting_row, slider_row, warning_box};
use super::draft::SettingsDraft;
use super::input::SettingsInputState;
use super::provider_form;
use super::widgets::editable_combo;
use crate::ai_bridge::AiBridge;
use crate::settings::CharacterSettings;
use bevy_ecs::world::World;
use std::sync::Arc;

/// Warns when the configured provider is not registered by any running
/// plugin; the selection stays visible but cannot take effect until the
/// plugin is enabled on the Tools & Plugins page.
fn provider_missing_hint(
    ui: &mut egui::Ui,
    provider: &str,
    catalog: Option<&[String]>,
    catalog_loaded: bool,
) {
    if catalog_loaded
        && !provider.is_empty()
        && provider != "none"
        && catalog.is_none_or(|kinds| !kinds.iter().any(|kind| kind == provider))
    {
        warning_box(
            ui,
            &i18n_embed_fl::fl!(crate::i18n::loader(), "audio-provider-not-running"),
        );
    }
}

fn provider_selector_control(
    ui: &mut egui::Ui,
    id_salt: &str,
    provider: &mut String,
    choices: &[(String, String)],
) -> bool {
    let description = provider_form::provider_description(provider);
    let group = provider_form::provider_display_group(provider);
    let combo = ui
        .horizontal_wrapped(|ui| editable_combo(ui, id_salt, provider, choices, 160.0))
        .inner;
    if let Some(description) = description {
        ui.add(egui::Label::new(egui::RichText::new(description).weak()).wrap());
    }
    if let Some(group) = group {
        ui.weak(format!("[{group}]"));
    }
    combo.commit_requested()
}

fn provider_setting_row(
    ui: &mut egui::Ui,
    id_salt: &str,
    title: &str,
    content: impl FnOnce(&mut egui::Ui),
) {
    ui.push_id(id_salt, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.label(title);
            ui.vertical(content);
        });
    });
}

pub fn render(
    ui: &mut egui::Ui,
    settings: &mut CharacterSettings,
    draft: &mut SettingsDraft,
    ai: &Arc<AiBridge>,
    input: &mut SettingsInputState,
    world: &mut World,
) {
    #[cfg(not(feature = "voice"))]
    let _ = world;
    input.provider_catalog.poll();
    if !input.provider_catalog.started() {
        input.provider_catalog.start(ai.fetch_provider_catalog());
    }
    if !input.plugin_snapshots.started() {
        input.plugin_snapshots.start(ai.fetch_plugin_snapshots());
    }
    input.plugin_snapshots.poll();
    if !input.artifact_snapshot.started() {
        input.artifact_snapshot.start(ai.fetch_artifact_snapshot());
    }
    input.artifact_snapshot.poll();
    super::artifact_card::poll_artifact_actions(input);
    #[cfg(feature = "voice")]
    if input.mic_devices.is_empty() {
        input.mic_devices = crate::audio::capture::list_input_device_names();
    }

    let mut ai_cfg = draft.section::<ene_ai::AiConfig>();
    let mut changed = false;
    let mut mic_device_changed = false;

    ui.vertical(|ui| {
        section_card(
            ui,
            "voice-tts",
            &i18n_embed_fl::fl!(crate::i18n::loader(), "audio-tts-section"),
            |ui| {
                ui.horizontal_wrapped(|ui| {
                    if ui
                        .button(i18n_embed_fl::fl!(
                            crate::i18n::loader(),
                            "audio-preset-kokoro"
                        ))
                        .on_hover_text(i18n_embed_fl::fl!(
                            crate::i18n::loader(),
                            "audio-preset-kokoro-tooltip"
                        ))
                        .clicked()
                    {
                        ai_cfg.tts.provider = "kokoro".to_string();
                        input.tts_provider = "kokoro".to_string();
                        changed = true;
                    }

                    if ui
                        .button(i18n_embed_fl::fl!(
                            crate::i18n::loader(),
                            "audio-preset-openai"
                        ))
                        .on_hover_text(i18n_embed_fl::fl!(
                            crate::i18n::loader(),
                            "audio-preset-openai-tooltip"
                        ))
                        .clicked()
                    {
                        ai_cfg.tts.provider = "openai_tts".to_string();
                        input.tts_provider = "openai_tts".to_string();
                        changed = true;
                    }

                    if ui
                        .button(i18n_embed_fl::fl!(
                            crate::i18n::loader(),
                            "audio-preset-none"
                        ))
                        .clicked()
                    {
                        ai_cfg.tts.provider = "none".to_string();
                        input.tts_provider = "none".to_string();
                        changed = true;
                    }
                });

                ui.add_space(6.0);
                provider_setting_row(
                    ui,
                    "tts_provider_row",
                    &i18n_embed_fl::fl!(crate::i18n::loader(), "audio-tts-provider"),
                    |ui| {
                        let mut choices: Vec<(String, String)> = Vec::new();
                        choices.push((
                            "none".to_string(),
                            i18n_embed_fl::fl!(crate::i18n::loader(), "audio-provider-none"),
                        ));
                        if let Some(catalog) =
                            input.provider_catalog.data.clone().flatten().as_ref()
                        {
                            choices.extend(catalog.tts.iter().map(|kind| {
                                (kind.clone(), provider_form::provider_display_name(kind))
                            }));
                        }
                        if !input.tts_provider.is_empty()
                            && input.tts_provider != "none"
                            && !choices
                                .iter()
                                .any(|(value, _)| value == &input.tts_provider)
                        {
                            choices.insert(
                                0,
                                (
                                    input.tts_provider.clone(),
                                    provider_form::provider_display_name(&input.tts_provider),
                                ),
                            );
                        }

                        let commit_requested = provider_selector_control(
                            ui,
                            "tts_provider_combo",
                            &mut input.tts_provider,
                            &choices,
                        );
                        if commit_requested {
                            let provider = input.tts_provider.trim().to_string();
                            if provider != ai_cfg.tts.provider {
                                ai_cfg.tts.provider.clone_from(&provider);
                                changed = true;
                            }
                        }
                        provider_missing_hint(
                            ui,
                            &input.tts_provider,
                            input
                                .provider_catalog
                                .data
                                .clone()
                                .flatten()
                                .as_ref()
                                .map(|catalog| catalog.tts.as_slice()),
                            input.provider_catalog.data.is_some(),
                        );
                    },
                );

                if ai_cfg.tts.provider != "none" && !ai_cfg.tts.provider.is_empty() {
                    ui.indent("tts_details", |ui| {
                        let plugin =
                            provider_form::plugin_name_for_provider_kind(&ai_cfg.tts.provider);
                        let artifacts = input.artifact_snapshot.data.clone().unwrap_or_default();
                        provider_form::render_provider_artifact_card(
                            ui,
                            ai,
                            input,
                            &artifacts,
                            &ai_cfg.tts.provider,
                        );
                        let snapshots = input.plugin_snapshots.data.clone().unwrap_or_default();
                        let rendered = provider_form::render_provider_config_form(
                            ui,
                            draft,
                            ai,
                            input,
                            &snapshots,
                            &plugin,
                            &ai_cfg.tts.provider,
                        );
                        changed |= rendered;
                        if !rendered && !snapshots.iter().any(|snapshot| snapshot.id == plugin) {
                            warning_box(
                                ui,
                                &i18n_embed_fl::fl!(
                                    crate::i18n::loader(),
                                    "audio-provider-not-running"
                                ),
                            );
                        }
                        provider_form::render_config_actions(
                            ui,
                            draft,
                            ai,
                            input,
                            snapshots.iter().find(|snapshot| snapshot.id == plugin),
                            &plugin,
                        );
                    });
                }
            },
        );

        section_card(
            ui,
            "voice-stt",
            &i18n_embed_fl::fl!(crate::i18n::loader(), "audio-stt-section"),
            |ui| {
                provider_setting_row(
                    ui,
                    "stt_provider_row",
                    &i18n_embed_fl::fl!(crate::i18n::loader(), "audio-stt-provider"),
                    |ui| {
                        let mut choices: Vec<(String, String)> = vec![(
                            "none".to_string(),
                            i18n_embed_fl::fl!(crate::i18n::loader(), "audio-provider-none"),
                        )];
                        if let Some(catalog) =
                            input.provider_catalog.data.clone().flatten().as_ref()
                        {
                            choices.extend(catalog.stt.iter().map(|kind| {
                                (kind.clone(), provider_form::provider_display_name(kind))
                            }));
                        }
                        if !input.stt_provider.is_empty()
                            && input.stt_provider != "none"
                            && !choices
                                .iter()
                                .any(|(value, _)| value == &input.stt_provider)
                        {
                            choices.insert(
                                0,
                                (
                                    input.stt_provider.clone(),
                                    provider_form::provider_display_name(&input.stt_provider),
                                ),
                            );
                        }

                        let commit_requested = provider_selector_control(
                            ui,
                            "stt_provider_combo",
                            &mut input.stt_provider,
                            &choices,
                        );
                        if commit_requested {
                            let provider = input.stt_provider.trim().to_string();
                            if provider != ai_cfg.stt.provider {
                                ai_cfg.stt.provider.clone_from(&provider);
                                changed = true;
                            }
                        }
                        provider_missing_hint(
                            ui,
                            &input.stt_provider,
                            input
                                .provider_catalog
                                .data
                                .clone()
                                .flatten()
                                .as_ref()
                                .map(|catalog| catalog.stt.as_slice()),
                            input.provider_catalog.data.is_some(),
                        );
                    },
                );
                if ai_cfg.stt.provider != "none" && !ai_cfg.stt.provider.is_empty() {
                    ui.indent("stt_details", |ui| {
                        let plugin =
                            provider_form::plugin_name_for_provider_kind(&ai_cfg.stt.provider);
                        let artifacts = input.artifact_snapshot.data.clone().unwrap_or_default();
                        provider_form::render_provider_artifact_card(
                            ui,
                            ai,
                            input,
                            &artifacts,
                            &ai_cfg.stt.provider,
                        );
                        let snapshots = input.plugin_snapshots.data.clone().unwrap_or_default();
                        let rendered = provider_form::render_provider_config_form(
                            ui,
                            draft,
                            ai,
                            input,
                            &snapshots,
                            &plugin,
                            &ai_cfg.stt.provider,
                        );
                        changed |= rendered;
                        if !rendered && !snapshots.iter().any(|snapshot| snapshot.id == plugin) {
                            warning_box(
                                ui,
                                &i18n_embed_fl::fl!(
                                    crate::i18n::loader(),
                                    "audio-provider-not-running"
                                ),
                            );
                        }
                        provider_form::render_config_actions(
                            ui,
                            draft,
                            ai,
                            input,
                            snapshots.iter().find(|snapshot| snapshot.id == plugin),
                            &plugin,
                        );
                    });
                }
            },
        );

        section_card(
            ui,
            "voice-mic",
            &i18n_embed_fl::fl!(crate::i18n::loader(), "audio-mic-section"),
            |ui| {
                setting_row(
                    ui,
                    "mic_device_row",
                    &i18n_embed_fl::fl!(crate::i18n::loader(), "audio-mic-device"),
                    "",
                    |ui| {
                        let mut device = settings.mic_device().unwrap_or_default();
                        let mut choices: Vec<(String, String)> = vec![(
                            String::new(),
                            i18n_embed_fl::fl!(crate::i18n::loader(), "audio-mic-default"),
                        )];
                        choices.extend(
                            input
                                .mic_devices
                                .iter()
                                .map(|name| (name.clone(), name.clone())),
                        );
                        if !device.is_empty() && !choices.iter().any(|(value, _)| value == &device)
                        {
                            choices.insert(0, (device.clone(), device.clone()));
                        }
                        let combo =
                            editable_combo(ui, "mic_device_combo", &mut device, &choices, 200.0);
                        if ui
                            .button(i18n_embed_fl::fl!(
                                crate::i18n::loader(),
                                "audio-mic-refresh"
                            ))
                            .clicked()
                        {
                            #[cfg(feature = "voice")]
                            {
                                input.mic_devices =
                                    crate::audio::capture::list_input_device_names();
                            }
                        }
                        if combo.commit_requested() {
                            settings.set_mic_device(if device.trim().is_empty() {
                                None
                            } else {
                                Some(device.trim().to_string())
                            });
                            mic_device_changed = true;
                        }
                    },
                );

                let mut vad_threshold = settings.vad_threshold();
                if slider_row(
                    ui,
                    "vad_threshold_row",
                    &i18n_embed_fl::fl!(crate::i18n::loader(), "audio-vad-threshold"),
                    "",
                    &mut vad_threshold,
                    0.0..=1.0,
                    0.05,
                    |v| format!("{v:.2}"),
                ) {
                    settings.set_vad_threshold(vad_threshold);
                    changed = true;
                }

                #[cfg(feature = "voice")]
                {
                    // Display the live capture state (a dead thread after a
                    // device unplug shows as disabled even though the config
                    // flag is set); writes still go to the persisted config.
                    let mut enabled = world
                        .get_resource::<crate::resource::beat_sync::BeatSyncRuntime>()
                        .is_some_and(crate::resource::beat_sync::BeatSyncRuntime::is_running);
                    if super::components::toggle_row(
                        ui,
                        "voice_beat_sync",
                        &i18n_embed_fl::fl!(crate::i18n::loader(), "beat-sync-enabled"),
                        &i18n_embed_fl::fl!(crate::i18n::loader(), "beat-sync-hint"),
                        &mut enabled,
                    ) {
                        settings.set_beat_sync_enabled(enabled);
                        settings.mark_dirty();
                        if let Err(e) = crate::audio::set_beat_sync_enabled(
                            world,
                            ai,
                            enabled,
                            settings.beat_sync_device(),
                        ) {
                            tracing::warn!(
                                component = "BeatSync",
                                error = %e,
                                "beat sync toggle failed"
                            );
                            // Roll the persisted setting back so a failed
                            // start (no loopback device, unsupported format)
                            // does not leave the feature enabled forever.
                            settings.set_beat_sync_enabled(!enabled);
                            settings.mark_dirty();
                        }
                    }
                }
            },
        );
    });

    if changed {
        draft.set_section(&ai_cfg);
    }

    #[cfg(feature = "voice")]
    if (changed || mic_device_changed)
        && let Some(mut audio) = world.get_resource_mut::<crate::audio::AudioState>()
    {
        audio.config = settings.config_clone();
        audio.mic_device = settings.mic_device();
    }
    #[cfg(not(feature = "voice"))]
    {
        let _ = mic_device_changed;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn voice_page_overflow(width: f32) -> f32 {
        let context = egui::Context::default();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(width, 700.0),
            )),
            ..Default::default()
        };
        let choices = vec![("openai_tts".to_string(), "OpenAI TTS (cloud)".to_string())];
        let mut provider = "openai_tts".to_string();
        let schema = json!({
            "type": "object",
            "properties": {
                "api_key": {
                    "oneOf": [
                        {"type": "string"},
                        {"type": "object", "properties": {
                            "source": {"type": "string", "enum": ["inline", "env", "auto"]},
                            "inline": {"type": "string"},
                            "env": {"type": "string"}
                        }}
                    ],
                    "x-ene-secret": true,
                    "description": "OpenAI API key or a credential descriptor with source set to inline, env, or auto"
                },
                "base_url": {
                    "type": "string",
                    "description": "API base URL override (defaults to https://api.openai.com/v1)",
                    "x-ene-ui": {"group": "connection", "impact": "runtime_reload"}
                },
                "model": {
                    "type": "string",
                    "enum": ["tts-1", "tts-1-hd"],
                    "description": "Speech API model ID (e.g. tts-1, tts-1-hd)",
                    "x-ene-ui": {"group": "voice", "impact": "runtime_reload"}
                },
                "voice": {
                    "type": "string",
                    "enum": ["alloy", "echo", "fable", "onyx", "nova", "shimmer"],
                    "description": "Default voice (e.g. alloy, nova, shimmer)",
                    "x-ene-ui": {"group": "voice", "impact": "runtime_reload"}
                },
                "speed": {
                    "type": "number",
                    "minimum": 0.25,
                    "maximum": 4.0,
                    "description": "Speech speed multiplier (0.25-4.0)",
                    "x-ene-ui": {"group": "voice", "impact": "runtime_reload"}
                }
            }
        });
        let mut value = json!({
            "api_key": "",
            "base_url": "https://api.openai.com/v1",
            "model": "tts-1",
            "voice": "alloy",
            "speed": 1.0
        });
        let mut overflow = 0.0_f32;
        let _output = context.run_ui(input, |ui| {
            super::super::components::page_header(
                ui,
                "Voice & Audio",
                "Speech-to-text, text-to-speech, microphone, and VAD.",
            );
            ui.separator();
            let output = egui::ScrollArea::vertical()
                .id_salt(("voice_page_layout", width.to_bits()))
                .hscroll(false)
                .auto_shrink([false; 2])
                .show_viewport(ui, |ui, viewport| {
                    ui.set_max_width(viewport.width());
                    ui.set_width(viewport.width());
                    let content_right = ui.max_rect().right();
                    ui.vertical(|ui| {
                        section_card(ui, "voice-tts-test", "Text-to-Speech (TTS)", |ui| {
                            ui.horizontal_wrapped(|ui| {
                                drop(ui.button("Kokoro 82M (Local, Recommended)"));
                                drop(ui.button("OpenAI TTS (Cloud)"));
                                drop(ui.button("Disabled"));
                            });
                            provider_setting_row(
                                ui,
                                "tts_provider_test",
                                "Text-to-speech provider",
                                |ui| {
                                    provider_selector_control(
                                        ui,
                                        "tts_provider_combo_test",
                                        &mut provider,
                                        &choices,
                                    );
                                },
                            );
                            ui.indent("tts_details_test", |ui| {
                                super::super::schema_form::schema_object_form(
                                    ui,
                                    &schema,
                                    &mut value,
                                    "plugins.list.openai-tts.config",
                                    super::super::schema_form::SchemaFormOptions {
                                        show_advanced: true,
                                        show_impact: true,
                                        epoch: 0,
                                        options: None,
                                    },
                                );
                            });
                        });
                    });
                    (ui.min_rect().right() - content_right).max(0.0)
                });
            overflow = output
                .inner
                .max((output.content_size.x - output.inner_rect.width()).max(0.0));
        });
        overflow
    }

    #[test]
    fn voice_page_stays_within_narrow_and_wide_viewports() {
        for width in [500.0, 560.0, 900.0] {
            let overflow = voice_page_overflow(width);
            assert!(
                overflow <= 0.5,
                "{width}-point Voice page overflowed by {overflow} points"
            );
        }
    }
}
