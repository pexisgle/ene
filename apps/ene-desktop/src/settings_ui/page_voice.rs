//! Voice settings page — Text-to-Speech (TTS), Speech-to-Text (STT), and Microphone/VAD configuration.

use super::input::SettingsInputState;
use crate::ai_bridge::AiBridge;
use crate::settings::CharacterSettings;
use bevy_ecs::world::World;
use std::sync::{Arc, Mutex, OnceLock};

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

/// Global download status tracker for the settings UI.
fn download_status() -> &'static Mutex<Option<Result<String, String>>> {
    static STATUS: OnceLock<Mutex<Option<Result<String, String>>>> = OnceLock::new();
    STATUS.get_or_init(|| Mutex::new(None))
}

pub fn render(
    ui: &mut egui::Ui,
    settings: &mut CharacterSettings,
    ai: &Arc<AiBridge>,
    input: &mut SettingsInputState,
    world: &mut World,
) {
    let mut ai_cfg = settings.config_section::<ene_ai::AiConfig>();
    let mut changed = false;
    let mut mic_device_changed = false;

    ui.vertical(|ui| {
        // ── 1. Text-to-Speech (TTS) ──
        ui.heading(i18n_embed_fl::fl!(
            crate::i18n::loader(),
            "audio-tts-section"
        ));

        // Presets card
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
                    ai_cfg.tts.provider = "openai".to_string();
                    ai_cfg.tts.model = "tts-1".to_string();
                    ai_cfg.tts.voice = "alloy".to_string();
                    ai_cfg.tts.language = "ja".to_string();
                    input.tts_provider = "openai".to_string();
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

        // Model Status & Download section for Kokoro
        if ai_cfg.tts.provider == "kokoro" {
            ui.group(|ui| {
                let model_path = ene_voice::default_kokoro_model_path();
                let voices_path = ene_voice::default_kokoro_voices_path();
                let ready = model_path.is_file() && voices_path.is_file();

                ui.horizontal(|ui| {
                    ui.label(i18n_embed_fl::fl!(
                        crate::i18n::loader(),
                        "audio-model-status"
                    ));
                    if ready {
                        ui.colored_label(
                            egui::Color32::LIGHT_GREEN,
                            i18n_embed_fl::fl!(crate::i18n::loader(), "audio-status-ready"),
                        );
                    } else {
                        ui.colored_label(
                            egui::Color32::GOLD,
                            i18n_embed_fl::fl!(crate::i18n::loader(), "audio-status-missing"),
                        );
                    }
                });

                let status_guard = download_status()
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if let Some(ref res) = *status_guard {
                    match res {
                        Ok(msg) => {
                            ui.colored_label(egui::Color32::LIGHT_GREEN, msg);
                        }
                        Err(err) => {
                            ui.colored_label(
                                egui::Color32::LIGHT_RED,
                                i18n_embed_fl::fl!(
                                    crate::i18n::loader(),
                                    "audio-download-error",
                                    error = err
                                ),
                            );
                        }
                    }
                }
                drop(status_guard);

                if !ready
                    && ui
                        .button(i18n_embed_fl::fl!(
                            crate::i18n::loader(),
                            "audio-download-button"
                        ))
                        .clicked()
                {
                    // `ensure_kokoro_files_exist` is now async (it performs
                    // network I/O); spawn it on the tokio runtime rather than
                    // a bare OS thread. `Handle::current()` works here because
                    // this is called from the UI thread, which holds a live
                    // `runtime.enter()` guard for the whole process (see
                    // `main.rs`) even though it is not itself a worker thread.
                    tokio::runtime::Handle::current().spawn(async move {
                        let res =
                            ene_voice::ensure_kokoro_files_exist(&model_path, &voices_path).await;
                        let mut lock = download_status()
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                        match res {
                            Ok(()) => {
                                *lock = Some(Ok(i18n_embed_fl::fl!(
                                    crate::i18n::loader(),
                                    "audio-download-success"
                                )));
                            }
                            Err(e) => {
                                *lock = Some(Err(e.to_string()));
                            }
                        }
                    });
                }
            });
            ui.add_space(4.0);
        }

        // Provider dropdown
        ui.horizontal(|ui| {
            ui.label(i18n_embed_fl::fl!(
                crate::i18n::loader(),
                "audio-tts-provider"
            ));

            let current_provider = ai_cfg.tts.provider.clone();
            let mut provider_selected = current_provider.as_str();

            egui::ComboBox::from_id_salt("tts_provider_combo")
                .selected_text(
                    if provider_selected == "none" || provider_selected.is_empty() {
                        i18n_embed_fl::fl!(crate::i18n::loader(), "audio-provider-none")
                    } else {
                        provider_selected.to_string()
                    },
                )
                .show_ui(ui, |ui| {
                    ui.selectable_value(
                        &mut provider_selected,
                        "none",
                        i18n_embed_fl::fl!(crate::i18n::loader(), "audio-provider-none"),
                    );
                    ui.selectable_value(&mut provider_selected, "kokoro", "kokoro (Local ONNX)");
                    ui.selectable_value(&mut provider_selected, "openai", "openai (Cloud API)");
                });

            if provider_selected != current_provider {
                ai_cfg.tts.provider = provider_selected.to_string();
                input.tts_provider = provider_selected.to_string();
                changed = true;
            }
        });

        // Advanced / detail controls for TTS when enabled
        if ai_cfg.tts.provider != "none" && !ai_cfg.tts.provider.is_empty() {
            ui.indent("tts_details", |ui| {
                // Voice character selection
                ui.horizontal(|ui| {
                    ui.label(i18n_embed_fl::fl!(crate::i18n::loader(), "audio-tts-voice"));

                    let mut selected_voice = input.tts_voice.clone();
                    let current_label = KOKORO_VOICE_PRESETS
                        .iter()
                        .find(|(id, _)| *id == selected_voice)
                        .map_or_else(|| selected_voice.as_str(), |(_, desc)| *desc);

                    let response = egui::ComboBox::from_id_salt("tts_voice_combo")
                        .selected_text(if current_label.is_empty() {
                            "af_heart (女性・標準)"
                        } else {
                            current_label
                        })
                        .show_ui(ui, |ui| {
                            for (id, desc) in KOKORO_VOICE_PRESETS {
                                ui.selectable_value(&mut selected_voice, (*id).to_string(), *desc);
                            }
                        });

                    if response.response.changed() && selected_voice != input.tts_voice {
                        input.tts_voice.clone_from(&selected_voice);
                        ai_cfg.tts.voice = selected_voice;
                        changed = true;
                    }

                    // Free text edit for custom voice name
                    let text_edit = ui
                        .add(egui::TextEdit::singleline(&mut input.tts_voice).desired_width(120.0));
                    if text_edit.changed() {
                        ai_cfg.tts.voice = input.tts_voice.trim().to_string();
                        changed = true;
                    }
                });

                // Speech Speed Slider
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

                // Language
                ui.horizontal(|ui| {
                    ui.label(i18n_embed_fl::fl!(
                        crate::i18n::loader(),
                        "audio-tts-language"
                    ));
                    let response = ui.add(
                        egui::TextEdit::singleline(&mut input.tts_language).desired_width(80.0),
                    );
                    if response.changed() {
                        ai_cfg.tts.language = input.tts_language.trim().to_string();
                        changed = true;
                    }
                });

                // Model name
                ui.horizontal(|ui| {
                    ui.label(i18n_embed_fl::fl!(crate::i18n::loader(), "audio-tts-model"));
                    let response = ui
                        .add(egui::TextEdit::singleline(&mut input.tts_model).desired_width(180.0));
                    if response.changed() {
                        ai_cfg.tts.model = input.tts_model.trim().to_string();
                        changed = true;
                    }
                });

                // Optional Model Path
                ui.horizontal(|ui| {
                    ui.label(i18n_embed_fl::fl!(
                        crate::i18n::loader(),
                        "audio-tts-model-path"
                    ));
                    let response = ui.add(
                        egui::TextEdit::singleline(&mut input.tts_model_path).desired_width(220.0),
                    );
                    if response.changed() {
                        ai_cfg.tts.model_path = if input.tts_model_path.trim().is_empty() {
                            None
                        } else {
                            Some(input.tts_model_path.trim().to_string())
                        };
                        changed = true;
                    }
                });

                // Optional Voices Path (#313: lives in the Kokoro plugin profile)
                ui.horizontal(|ui| {
                    ui.label(i18n_embed_fl::fl!(
                        crate::i18n::loader(),
                        "audio-tts-voices-path"
                    ));
                    let response = ui.add(
                        egui::TextEdit::singleline(&mut input.tts_voices_path).desired_width(220.0),
                    );
                    if response.changed() {
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

        // Provider selection
        ui.horizontal(|ui| {
            ui.label(i18n_embed_fl::fl!(
                crate::i18n::loader(),
                "audio-stt-provider"
            ));

            let current_provider = ai_cfg.stt.provider.clone();
            let mut provider_selected = current_provider.as_str();

            egui::ComboBox::from_id_salt("stt_provider_combo")
                .selected_text(
                    if provider_selected == "none" || provider_selected.is_empty() {
                        i18n_embed_fl::fl!(crate::i18n::loader(), "audio-provider-none")
                    } else {
                        provider_selected.to_string()
                    },
                )
                .show_ui(ui, |ui| {
                    ui.selectable_value(
                        &mut provider_selected,
                        "none",
                        i18n_embed_fl::fl!(crate::i18n::loader(), "audio-provider-none"),
                    );
                    ui.selectable_value(&mut provider_selected, "whisper", "whisper (Local)");
                });

            if provider_selected != current_provider {
                ai_cfg.stt.provider = provider_selected.to_string();
                input.stt_provider = provider_selected.to_string();
                changed = true;
            }
        });

        if ai_cfg.stt.provider != "none" && !ai_cfg.stt.provider.is_empty() {
            ui.indent("stt_details", |ui| {
                // Model name
                ui.horizontal(|ui| {
                    ui.label(i18n_embed_fl::fl!(crate::i18n::loader(), "audio-stt-model"));
                    let response = ui
                        .add(egui::TextEdit::singleline(&mut input.stt_model).desired_width(180.0));
                    if response.changed() {
                        ai_cfg.stt.model = input.stt_model.trim().to_string();
                        changed = true;
                    }
                });

                // Language
                ui.horizontal(|ui| {
                    ui.label(i18n_embed_fl::fl!(
                        crate::i18n::loader(),
                        "audio-stt-language"
                    ));
                    let response = ui.add(
                        egui::TextEdit::singleline(&mut input.stt_language).desired_width(80.0),
                    );
                    if response.changed() {
                        ai_cfg.stt.language = input.stt_language.trim().to_string();
                        changed = true;
                    }
                });

                // Optional Model Path
                ui.horizontal(|ui| {
                    ui.label(i18n_embed_fl::fl!(
                        crate::i18n::loader(),
                        "audio-stt-model-path"
                    ));
                    let response = ui.add(
                        egui::TextEdit::singleline(&mut input.stt_model_path).desired_width(220.0),
                    );
                    if response.changed() {
                        ai_cfg.stt.model_path = if input.stt_model_path.trim().is_empty() {
                            None
                        } else {
                            Some(input.stt_model_path.trim().to_string())
                        };
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

        // Microphone device
        ui.horizontal(|ui| {
            ui.label(i18n_embed_fl::fl!(
                crate::i18n::loader(),
                "audio-mic-device"
            ));
            let mut device = settings.mic_device().unwrap_or_default();
            if ui
                .add(egui::TextEdit::singleline(&mut device).desired_width(200.0))
                .on_hover_text(i18n_embed_fl::fl!(
                    crate::i18n::loader(),
                    "audio-mic-default"
                ))
                .changed()
            {
                settings.set_mic_device(if device.trim().is_empty() {
                    None
                } else {
                    Some(device.trim().to_string())
                });
                mic_device_changed = true;
            }
        });

        // VAD threshold slider
        ui.horizontal(|ui| {
            ui.label(i18n_embed_fl::fl!(
                crate::i18n::loader(),
                "audio-vad-threshold"
            ));
            let mut threshold = ai_cfg.vad.threshold;
            if ui
                .add(egui::Slider::new(&mut threshold, 0.0..=1.0))
                .changed()
            {
                ai_cfg.vad.threshold = threshold;
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
