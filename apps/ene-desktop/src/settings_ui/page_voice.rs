//! Voice settings page — Text-to-Speech (TTS), Speech-to-Text (STT), and Microphone/VAD configuration.

use super::input::SettingsInputState;
use super::widgets::editable_combo;
use crate::ai_bridge::AiBridge;
use crate::settings::CharacterSettings;
use bevy_ecs::world::World;
use std::sync::Arc;

/// Known Kokoro TTS voice presets with human-readable Japanese descriptions.
const KOKORO_VOICE_PRESETS: &[(&str, &str)] = &[
    ("af_heart", "af_heart (女性・標準)"),
    ("af_bella", "af_bella (女性・明瞭)"),
    ("af_nicole", "af_nicole (女性・落ち着いた)"),
    ("af_sky", "af_sky (女性・明るい)"),
    ("af_alloy", "af_alloy (女性・中性)"),
    ("af_aoede", "af_aoede (女性・表現力豊かな)"),
    ("af_kore", "af_kore (女性・ナレーション)"),
    ("af_nova", "af_nova (女性・エネルギッシュ)"),
    ("af_river", "af_river (女性・ソフト)"),
    ("af_sarah", "af_sarah (女性・アナウンス)"),
    ("am_adam", "am_adam (男性・標準)"),
    ("am_echo", "am_echo (男性・落ち着いた)"),
    ("jf_alpha", "jf_alpha (日本語女性)"),
    ("jf_gongitsune", "jf_gongitsune (日本語童話風)"),
];

/// `OpenAI` Speech API voices advertised by the `openai-tts` plugin.
const OPENAI_TTS_VOICES: &[&str] = &["alloy", "echo", "fable", "onyx", "nova", "shimmer"];
/// `OpenAI` Speech API models advertised by the `openai-tts` plugin.
const OPENAI_TTS_MODELS: &[&str] = &["tts-1", "tts-1-hd"];
/// Language codes offered as one-click choices for TTS / STT; the free-form
/// editor accepts any other BCP-47 code.
const LANGUAGE_CHOICES: &[&str] = &["ja", "en", "zh", "ko", "fr", "de", "es", "it", "pt"];

/// TTS voice presets for the selected provider, as (value, label) pairs.
fn tts_voice_choices(provider: &str, current: &str) -> Vec<(String, String)> {
    let presets: Vec<(String, String)> = match provider {
        "kokoro" => KOKORO_VOICE_PRESETS
            .iter()
            .map(|(id, desc)| ((*id).to_string(), (*desc).to_string()))
            .collect(),
        "openai_tts" => OPENAI_TTS_VOICES
            .iter()
            .map(|v| ((*v).to_string(), (*v).to_string()))
            .collect(),
        _ => Vec::new(),
    };
    if !current.is_empty() && !presets.iter().any(|(value, _)| value == current) {
        let mut with_current = vec![(current.to_string(), current.to_string())];
        with_current.extend(presets);
        return with_current;
    }
    presets
}

/// TTS model presets for the selected provider, as (value, label) pairs.
fn tts_model_choices(provider: &str, current: &str) -> Vec<(String, String)> {
    let presets: Vec<(String, String)> = match provider {
        "kokoro" => vec![(
            "kokoro-v1_0.onnx".to_string(),
            "kokoro-v1_0.onnx (local)".to_string(),
        )],
        "openai_tts" => OPENAI_TTS_MODELS
            .iter()
            .map(|m| ((*m).to_string(), (*m).to_string()))
            .collect(),
        _ => Vec::new(),
    };
    if !current.is_empty() && !presets.iter().any(|(value, _)| value == current) {
        let mut with_current = vec![(current.to_string(), current.to_string())];
        with_current.extend(presets);
        return with_current;
    }
    presets
}

fn language_choices(current: &str) -> Vec<(String, String)> {
    let mut choices: Vec<(String, String)> = LANGUAGE_CHOICES
        .iter()
        .map(|code| ((*code).to_string(), (*code).to_string()))
        .collect();
    if !current.is_empty() && !choices.iter().any(|(value, _)| value == current) {
        choices.insert(0, (current.to_string(), current.to_string()));
    }
    choices
}

/// Whisper GGUF model filenames offered as quick picks for the STT model
/// field. The field is a path fallback per the whisper plugin contract, so
/// the free-form editor remains the primary input.
fn stt_model_choices(current: &str) -> Vec<(String, String)> {
    let presets = [
        "ggml-tiny.bin",
        "ggml-base.bin",
        "ggml-small.bin",
        "ggml-medium.bin",
        "ggml-large-v3-turbo.bin",
    ];
    let mut choices: Vec<(String, String)> = presets
        .iter()
        .map(|name| ((*name).to_string(), (*name).to_string()))
        .collect();
    if !current.is_empty() && !choices.iter().any(|(value, _)| value == current) {
        choices.insert(0, (current.to_string(), current.to_string()));
    }
    choices
}

pub fn render(
    ui: &mut egui::Ui,
    settings: &mut CharacterSettings,
    ai: &Arc<AiBridge>,
    input: &mut SettingsInputState,
    world: &mut World,
) {
    if input.provider_catalog.is_none() {
        input.provider_catalog = ai.provider_catalog_blocking().ok();
    }
    #[cfg(feature = "voice")]
    if input.mic_devices.is_empty() {
        input.mic_devices = crate::audio::capture::list_input_device_names();
    }

    let mut ai_cfg = settings.config_section::<ene_ai::AiConfig>();
    let mut changed = false;
    let mut mic_device_changed = false;

    ui.vertical(|ui| {
        // ── 1. Text-to-Speech (TTS) ──
        ui.heading(i18n_embed_fl::fl!(
            crate::i18n::loader(),
            "audio-tts-section"
        ));
        ui.group(|ui| {
            ui.label(i18n_embed_fl::fl!(
                crate::i18n::loader(),
                "audio-tts-presets"
            ));
            ui.horizontal(|ui| {
                if ui
                    .button(i18n_embed_fl::fl!(
                        crate::i18n::loader(),
                        "audio-preset-kokoro"
                    ))
                    .on_hover_text("Kokoro-82M ローカル ONNX エンジンを一括設定")
                    .clicked()
                {
                    ai_cfg.tts.provider = "kokoro".to_string();
                    ai_cfg.tts.model = "kokoro-v1_0.onnx".to_string();
                    ai_cfg.tts.voice = "af_heart".to_string();
                    ai_cfg.tts.language = "ja".to_string();
                    input.tts_provider = "kokoro".to_string();
                    input.tts_model = "kokoro-v1_0.onnx".to_string();
                    input.tts_voice = "af_heart".to_string();
                    input.tts_language = "ja".to_string();
                    changed = true;
                }

                if ui
                    .button(i18n_embed_fl::fl!(
                        crate::i18n::loader(),
                        "audio-preset-openai"
                    ))
                    .on_hover_text("OpenAI TTS クラウド API を一括設定")
                    .clicked()
                {
                    ai_cfg.tts.provider = "openai_tts".to_string();
                    ai_cfg.tts.model = "tts-1".to_string();
                    ai_cfg.tts.voice = "alloy".to_string();
                    ai_cfg.tts.language = "ja".to_string();
                    input.tts_provider = "openai_tts".to_string();
                    input.tts_model = "tts-1".to_string();
                    input.tts_voice = "alloy".to_string();
                    input.tts_language = "ja".to_string();
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
        });

        ui.add_space(4.0);

        ui.horizontal(|ui| {
            ui.label(i18n_embed_fl::fl!(
                crate::i18n::loader(),
                "audio-tts-provider"
            ));

            let mut choices: Vec<(String, String)> = Vec::new();
            choices.push((
                "none".to_string(),
                i18n_embed_fl::fl!(crate::i18n::loader(), "audio-provider-none"),
            ));
            if let Some(catalog) = input.provider_catalog.as_ref() {
                choices.extend(catalog.tts.iter().map(|kind| (kind.clone(), kind.clone())));
            }
            if !input.tts_provider.is_empty()
                && input.tts_provider != "none"
                && !choices
                    .iter()
                    .any(|(value, _)| value == &input.tts_provider)
            {
                choices.insert(0, (input.tts_provider.clone(), input.tts_provider.clone()));
            }

            let combo = editable_combo(
                ui,
                "tts_provider_combo",
                &mut input.tts_provider,
                &choices,
                160.0,
            );
            if combo.selection_changed || combo.response.lost_focus() {
                let provider = input.tts_provider.trim().to_string();
                if provider != ai_cfg.tts.provider {
                    ai_cfg.tts.provider.clone_from(&provider);
                    changed = true;
                }
            }
        });

        if ai_cfg.tts.provider != "none" && !ai_cfg.tts.provider.is_empty() {
            ui.indent("tts_details", |ui| {
                ui.horizontal(|ui| {
                    ui.label(i18n_embed_fl::fl!(crate::i18n::loader(), "audio-tts-voice"));

                    let choices = tts_voice_choices(&ai_cfg.tts.provider, &input.tts_voice);
                    let combo = editable_combo(
                        ui,
                        "tts_voice_combo",
                        &mut input.tts_voice,
                        &choices,
                        120.0,
                    );
                    if combo.response.changed() || combo.selection_changed {
                        ai_cfg.tts.voice = input.tts_voice.trim().to_string();
                        changed = true;
                    }
                });

                ui.horizontal(|ui| {
                    ui.label(i18n_embed_fl::fl!(crate::i18n::loader(), "audio-tts-speed"));
                    let mut speed = ai_cfg.tts.speed;
                    if ui
                        .add(egui::Slider::new(&mut speed, 0.5..=2.0).step_by(0.1))
                        .changed()
                    {
                        ai_cfg.tts.speed = speed;
                        changed = true;
                    }
                });

                ui.horizontal(|ui| {
                    ui.label(i18n_embed_fl::fl!(
                        crate::i18n::loader(),
                        "audio-tts-language"
                    ));
                    let choices = language_choices(&input.tts_language);
                    let combo = editable_combo(
                        ui,
                        "tts_language_combo",
                        &mut input.tts_language,
                        &choices,
                        80.0,
                    );
                    if combo.response.changed() || combo.selection_changed {
                        ai_cfg.tts.language = input.tts_language.trim().to_string();
                        changed = true;
                    }
                });

                ui.horizontal(|ui| {
                    ui.label(i18n_embed_fl::fl!(crate::i18n::loader(), "audio-tts-model"));
                    let choices = tts_model_choices(&ai_cfg.tts.provider, &input.tts_model);
                    let combo = editable_combo(
                        ui,
                        "tts_model_combo",
                        &mut input.tts_model,
                        &choices,
                        180.0,
                    );
                    if combo.response.changed() || combo.selection_changed {
                        ai_cfg.tts.model = input.tts_model.trim().to_string();
                        changed = true;
                    }
                });

                ui.horizontal(|ui| {
                    ui.label(i18n_embed_fl::fl!(
                        crate::i18n::loader(),
                        "audio-tts-model-path"
                    ));
                    if super::widgets::path_row(ui, &mut input.tts_model_path, 220.0) {
                        ai_cfg.tts.model_path = if input.tts_model_path.trim().is_empty() {
                            None
                        } else {
                            Some(input.tts_model_path.trim().to_string())
                        };
                        changed = true;
                    }
                });

                ui.horizontal(|ui| {
                    ui.label(i18n_embed_fl::fl!(
                        crate::i18n::loader(),
                        "audio-tts-voices-path"
                    ));
                    if super::widgets::path_row(ui, &mut input.tts_voices_path, 220.0) {
                        settings.set_kokoro_voices_path(&input.tts_voices_path);
                        changed = true;
                    }
                });
            });
        }

        ui.separator();

        // ── 2. Speech-to-Text (STT) ──
        ui.heading(i18n_embed_fl::fl!(
            crate::i18n::loader(),
            "audio-stt-section"
        ));

        ui.horizontal(|ui| {
            ui.label(i18n_embed_fl::fl!(
                crate::i18n::loader(),
                "audio-stt-provider"
            ));

            let mut choices: Vec<(String, String)> = vec![(
                "none".to_string(),
                i18n_embed_fl::fl!(crate::i18n::loader(), "audio-provider-none"),
            )];
            if let Some(catalog) = input.provider_catalog.as_ref() {
                choices.extend(catalog.stt.iter().map(|kind| (kind.clone(), kind.clone())));
            }
            if !input.stt_provider.is_empty()
                && input.stt_provider != "none"
                && !choices
                    .iter()
                    .any(|(value, _)| value == &input.stt_provider)
            {
                choices.insert(0, (input.stt_provider.clone(), input.stt_provider.clone()));
            }

            let combo = editable_combo(
                ui,
                "stt_provider_combo",
                &mut input.stt_provider,
                &choices,
                160.0,
            );
            if combo.selection_changed || combo.response.lost_focus() {
                let provider = input.stt_provider.trim().to_string();
                if provider != ai_cfg.stt.provider {
                    ai_cfg.stt.provider.clone_from(&provider);
                    changed = true;
                }
            }
        });
        if ai_cfg.stt.provider != "none" && !ai_cfg.stt.provider.is_empty() {
            ui.indent("stt_details", |ui| {
                ui.horizontal(|ui| {
                    ui.label(i18n_embed_fl::fl!(crate::i18n::loader(), "audio-stt-model"));
                    let choices = stt_model_choices(&input.stt_model);
                    let combo = editable_combo(
                        ui,
                        "stt_model_combo",
                        &mut input.stt_model,
                        &choices,
                        180.0,
                    );
                    if combo.response.changed() || combo.selection_changed {
                        ai_cfg.stt.model = input.stt_model.trim().to_string();
                        changed = true;
                    }
                });

                ui.horizontal(|ui| {
                    ui.label(i18n_embed_fl::fl!(
                        crate::i18n::loader(),
                        "audio-stt-language"
                    ));
                    let choices = language_choices(&input.stt_language);
                    let combo = editable_combo(
                        ui,
                        "stt_language_combo",
                        &mut input.stt_language,
                        &choices,
                        80.0,
                    );
                    if combo.response.changed() || combo.selection_changed {
                        ai_cfg.stt.language = input.stt_language.trim().to_string();
                        changed = true;
                    }
                });

                ui.horizontal(|ui| {
                    ui.label(i18n_embed_fl::fl!(
                        crate::i18n::loader(),
                        "audio-stt-model-path"
                    ));
                    let mut model_path = settings.whisper_model_path();
                    if super::widgets::path_row(ui, &mut model_path, 220.0) {
                        settings.set_whisper_model_path(&model_path);
                        changed = true;
                    }
                });
            });
        }

        ui.separator();

        // ── 3. Microphone & VAD ──
        ui.heading(i18n_embed_fl::fl!(
            crate::i18n::loader(),
            "audio-mic-section"
        ));

        ui.horizontal(|ui| {
            ui.label(i18n_embed_fl::fl!(
                crate::i18n::loader(),
                "audio-mic-device"
            ));

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
            if !device.is_empty() && !choices.iter().any(|(value, _)| value == &device) {
                choices.insert(0, (device.clone(), device.clone()));
            }
            let combo = editable_combo(ui, "mic_device_combo", &mut device, &choices, 200.0);
            if ui
                .button(i18n_embed_fl::fl!(
                    crate::i18n::loader(),
                    "audio-mic-refresh"
                ))
                .on_hover_text(i18n_embed_fl::fl!(
                    crate::i18n::loader(),
                    "audio-mic-default"
                ))
                .clicked()
            {
                #[cfg(feature = "voice")]
                {
                    input.mic_devices = crate::audio::capture::list_input_device_names();
                }
            }
            if combo.response.changed() || combo.selection_changed {
                settings.set_mic_device(if device.trim().is_empty() {
                    None
                } else {
                    Some(device.trim().to_string())
                });
                mic_device_changed = true;
            }
        });

        ui.horizontal(|ui| {
            ui.label(i18n_embed_fl::fl!(
                crate::i18n::loader(),
                "audio-vad-threshold"
            ));
            let mut threshold = settings.vad_threshold();
            if ui
                .add(egui::Slider::new(&mut threshold, 0.0..=1.0))
                .changed()
            {
                settings.set_vad_threshold(threshold);
                changed = true;
            }
        });
    });

    if changed {
        settings.set_config_section(&ai_cfg);
        settings.mark_dirty();
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

    if changed {
        let mind = settings.config_section::<ene_mind::MindConfig>();
        let store = settings.config_section::<ene_store::StoreConfig>();
        let tools = settings.config_section::<ene_plugin_host::PluginConfig>();
        let rag = settings.config_section::<ene_rag::ToolRagConfig>();
        ai.sync_feature_runtime(&mind, &store, &tools, &rag);
    }
}
