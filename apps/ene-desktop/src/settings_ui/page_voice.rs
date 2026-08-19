use std::sync::Arc;

use crate::core_session::CoreSession;
use crate::settings::CharacterSettings;

use super::components::{section_card, toggle_row};
use super::draft::SettingsDraft;
use super::input::SettingsInputState;
use super::provider_form::{
    ProviderInfo, catalog_from_settings, ids_with_seam, plugin_combo, plugin_needs_key,
    provider_description, sidecar_fields,
};
use serde_json::{Value, json};

pub fn render(
    ui: &mut egui::Ui,
    settings: &mut CharacterSettings,
    draft: &mut SettingsDraft,
    ai: &Arc<CoreSession>,
    input: &mut SettingsInputState,
    world: &mut bevy_ecs::world::World,
) {
    input.core_settings.poll();
    if !input.core_settings.started() {
        input.core_settings.start(ai.fetch_core_settings());
    }
    if let Some(Ok(core)) = &input.core_settings.data
        && draft.editing().section_value("ai").is_none()
        && let Some(ai_value) = core.pointer("/effective/ai")
    {
        draft.seed_core_section("ai", ai_value.clone());
        input.ai_tts_key_set = core
            .pointer("/effective/ai_tts_key_set")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        input.ai_stt_key_set = core
            .pointer("/effective/ai_stt_key_set")
            .and_then(Value::as_bool)
            .unwrap_or(false);
    }

    let catalog = catalog_from_settings(
        input
            .core_settings
            .data
            .as_ref()
            .and_then(|r| r.as_ref().ok()),
    );

    section_card(
        ui,
        "voice-tts",
        &i18n_embed_fl::fl!(crate::i18n::loader(), "audio-tts-section"),
        |ui| {
            render_audio_binding(
                ui,
                draft,
                input,
                "tts",
                &ids_with_seam(&catalog, "seam.tts"),
                &catalog,
            );
        },
    );
    section_card(
        ui,
        "voice-stt",
        &i18n_embed_fl::fl!(crate::i18n::loader(), "audio-stt-section"),
        |ui| {
            render_audio_binding(
                ui,
                draft,
                input,
                "stt",
                &ids_with_seam(&catalog, "seam.stt"),
                &catalog,
            );
        },
    );
    section_card(
        ui,
        "voice-mic",
        &i18n_embed_fl::fl!(crate::i18n::loader(), "audio-mic-section"),
        |ui| {
            #[cfg(feature = "voice")]
            {
                if input.mic_devices.is_empty() {
                    input.mic_devices = crate::audio::list_input_device_names();
                }
                let mut selected = settings.mic_device();
                let label = selected
                    .clone()
                    .unwrap_or_else(|| i18n_embed_fl::fl!(crate::i18n::loader(), "match-window"));
                egui::ComboBox::from_id_salt("mic_device")
                    .selected_text(label)
                    .show_ui(ui, |ui| {
                        if ui
                            .selectable_label(
                                selected.is_none(),
                                i18n_embed_fl::fl!(crate::i18n::loader(), "match-window"),
                            )
                            .clicked()
                        {
                            selected = None;
                        }
                        for name in &input.mic_devices {
                            if ui
                                .selectable_label(selected.as_deref() == Some(name), name)
                                .clicked()
                            {
                                selected = Some(name.clone());
                            }
                        }
                    });
                if selected != settings.mic_device() {
                    settings.set_mic_device(selected);
                    settings.mark_dirty();
                }

                let mut beat_sync = settings
                    .config_section::<crate::settings::DesktopSection>()
                    .beat_sync
                    .enabled;
                if toggle_row(
                    ui,
                    "beat_sync",
                    &i18n_embed_fl::fl!(crate::i18n::loader(), "beat-sync-enabled"),
                    &i18n_embed_fl::fl!(crate::i18n::loader(), "beat-sync-hint"),
                    &mut beat_sync,
                ) {
                    settings.set_beat_sync_enabled(beat_sync);
                    settings.mark_dirty();
                    if let Err(err) = crate::audio::set_beat_sync_enabled(
                        world,
                        ai,
                        beat_sync,
                        settings.beat_sync_device(),
                    ) {
                        tracing::warn!(error = %err, "beat sync toggle failed");
                    }
                }
            }
            #[cfg(not(feature = "voice"))]
            {
                drop((ai, world));
                ui.weak(i18n_embed_fl::fl!(
                    crate::i18n::loader(),
                    "voice-feature-disabled"
                ));
            }
        },
    );
}

fn render_audio_binding(
    ui: &mut egui::Ui,
    draft: &mut SettingsDraft,
    input: &mut SettingsInputState,
    task: &str,
    plugins: &[String],
    catalog: &[ProviderInfo],
) {
    let mut binding = draft
        .editing()
        .section_value("ai")
        .and_then(|ai| ai.pointer(&format!("/tasks/{task}")).cloned())
        .unwrap_or_else(|| json!({ "plugin": "", "model": "" }));
    let mut plugin = binding
        .get("plugin")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    plugin_combo(ui, &format!("voice-{task}-plugin"), &mut plugin, plugins);
    if let Some(desc) = provider_description(&plugin) {
        ui.weak(desc);
    }
    let mut model = binding
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    let mut voice = binding
        .get("voice")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    let mut base_url = binding
        .get("base_url")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    ui.label(i18n_embed_fl::fl!(crate::i18n::loader(), "ai-model-label"));
    let model_changed = ui
        .add(egui::TextEdit::singleline(&mut model).desired_width(f32::INFINITY))
        .changed();
    if task == "tts" {
        ui.label(i18n_embed_fl::fl!(crate::i18n::loader(), "ai-voice-label"));
    }
    let voice_changed = task == "tts"
        && ui
            .add(egui::TextEdit::singleline(&mut voice).desired_width(f32::INFINITY))
            .changed();
    ui.label(i18n_embed_fl::fl!(
        crate::i18n::loader(),
        "ai-base-url-label"
    ));
    let url_changed = ui
        .add(egui::TextEdit::singleline(&mut base_url).desired_width(f32::INFINITY))
        .changed();
    let sidecar_changed = sidecar_fields(ui, &mut binding, catalog);
    let mut key_changed = false;
    if plugin_needs_key(catalog, &plugin) {
        ui.label(i18n_embed_fl::fl!(
            crate::i18n::loader(),
            "ai-api-key-label"
        ));
        let (buffer, set) = if task == "tts" {
            (&mut input.ai_tts_key, input.ai_tts_key_set)
        } else {
            (&mut input.ai_stt_key, input.ai_stt_key_set)
        };
        if set && buffer.is_empty() {
            ui.weak(i18n_embed_fl::fl!(crate::i18n::loader(), "ai-key-set"));
        }
        key_changed = ui
            .add(
                egui::TextEdit::singleline(buffer)
                    .password(true)
                    .desired_width(f32::INFINITY),
            )
            .changed();
    }
    if plugin != binding.get("plugin").and_then(Value::as_str).unwrap_or("")
        || model_changed
        || voice_changed
        || url_changed
        || sidecar_changed
        || key_changed
    {
        binding["plugin"] = json!(plugin);
        binding["model"] = json!(model);
        binding["base_url"] = json!(base_url);
        if task == "tts" {
            binding["voice"] = json!(voice);
            if !input.ai_tts_key.is_empty() {
                binding["api_key"] = json!(input.ai_tts_key.clone());
            }
        } else if !input.ai_stt_key.is_empty() {
            binding["api_key"] = json!(input.ai_stt_key.clone());
        }
        let mut ai = draft
            .editing()
            .section_value("ai")
            .unwrap_or_else(|| json!({ "tasks": {} }));
        ai["tasks"][task] = binding;
        draft.set_section_value("ai", ai);
    }
}
