//! Features settings page — mind / tools toggles and proactive policy knobs.
//!
//! Provider / embedding settings stay on the AI tab. This page owns the
//! public-schema switches and proactive timing / source policy.

use crate::ai_bridge::AiBridge;
use crate::settings::CharacterSettings;
use bevy_ecs::world::World;
use ene_mind::WindowTitleLevel;
use ene_plugin_host::PluginConfig;
use ene_rag::ToolRagConfig;
use std::sync::Arc;

/// Known tool binary names shown even when absent from the saved map.
const DEFAULT_TOOL_NAMES: &[&str] = &["app", "browser", "fs", "utility", "web"];

pub fn render(
    ui: &mut egui::Ui,
    settings: &mut CharacterSettings,
    ai: &Arc<AiBridge>,
    world: &mut World,
) {
    ui.vertical(|ui| {
        ui.weak(i18n_embed_fl::fl!(crate::i18n::loader(), "features-hint"));
        ui.separator();

        render_mind(ui, settings, ai);
        ui.separator();
        render_tools(ui, settings, ai);
        ui.separator();
        render_audio(ui, settings, ai, world);
    });
}

fn render_mind(ui: &mut egui::Ui, settings: &mut CharacterSettings, ai: &Arc<AiBridge>) {
    ui.label(i18n_embed_fl::fl!(crate::i18n::loader(), "features-mind"));

    let mut memory = settings.config_section::<ene_store::StoreConfig>();
    let mut mind = settings.config_section::<ene_mind::MindConfig>();

    let mut memory_enabled = memory.enabled;
    if ui
        .checkbox(
            &mut memory_enabled,
            i18n_embed_fl::fl!(crate::i18n::loader(), "enable-long-term-memory"),
        )
        .changed()
    {
        memory.enabled = memory_enabled;
        settings.set_config_section(&memory);
        settings.mark_dirty();
        sync_features(settings, ai);
    }

    let mut emotion_enabled = mind.emotion.enabled;
    if ui
        .checkbox(
            &mut emotion_enabled,
            i18n_embed_fl::fl!(crate::i18n::loader(), "enable-emotion"),
        )
        .changed()
    {
        mind.emotion.enabled = emotion_enabled;
        persist_mind(settings, ai, &mind);
    }

    ui.separator();
    ui.label(i18n_embed_fl::fl!(
        crate::i18n::loader(),
        "proactive-speech"
    ));

    let mut proactive_enabled = mind.proactive.enabled;
    if ui
        .checkbox(
            &mut proactive_enabled,
            i18n_embed_fl::fl!(crate::i18n::loader(), "proactive-enabled"),
        )
        .changed()
    {
        mind.proactive.enabled = proactive_enabled;
        persist_mind(settings, ai, &mind);
    }

    ui.add_enabled_ui(mind.proactive.enabled, |ui| {
        ui.horizontal(|ui| {
            ui.label(i18n_embed_fl::fl!(
                crate::i18n::loader(),
                "proactive-interval"
            ));
            let mut value = mind.proactive.interval_seconds as i32;
            if ui
                .add(egui::DragValue::new(&mut value).range(1..=3600))
                .changed()
            {
                mind.proactive.interval_seconds = value.max(1) as u64;
                persist_mind(settings, ai, &mind);
            }
        });

        ui.horizontal(|ui| {
            ui.label(i18n_embed_fl::fl!(
                crate::i18n::loader(),
                "proactive-cooldown"
            ));
            let mut value = mind.proactive.cooldown_seconds as i32;
            if ui
                .add(egui::DragValue::new(&mut value).range(0..=86_400))
                .changed()
            {
                mind.proactive.cooldown_seconds = value.max(0) as u64;
                persist_mind(settings, ai, &mind);
            }
        });

        ui.horizontal(|ui| {
            ui.label(i18n_embed_fl::fl!(
                crate::i18n::loader(),
                "proactive-min-idle"
            ));
            let mut value = mind.proactive.min_idle_seconds as i32;
            if ui
                .add(egui::DragValue::new(&mut value).range(0..=86_400))
                .changed()
            {
                mind.proactive.min_idle_seconds = value.max(0) as u64;
                persist_mind(settings, ai, &mind);
            }
        });

        ui.label(i18n_embed_fl::fl!(
            crate::i18n::loader(),
            "proactive-sources"
        ));

        let mut conversation = mind.proactive.sources.conversation;
        if ui
            .checkbox(
                &mut conversation,
                i18n_embed_fl::fl!(crate::i18n::loader(), "proactive-source-conversation"),
            )
            .changed()
        {
            mind.proactive.sources.conversation = conversation;
            persist_mind(settings, ai, &mind);
        }

        let mut activity = mind.proactive.sources.activity;
        if ui
            .checkbox(
                &mut activity,
                i18n_embed_fl::fl!(crate::i18n::loader(), "proactive-source-activity"),
            )
            .changed()
        {
            mind.proactive.sources.activity = activity;
            persist_mind(settings, ai, &mind);
        }

        ui.add_enabled_ui(mind.proactive.sources.activity, |ui| {
            render_window_title_level(ui, settings, ai, &mut mind);
        });

        let mut screen = mind.proactive.sources.screen_summary;
        if ui
            .checkbox(
                &mut screen,
                i18n_embed_fl::fl!(crate::i18n::loader(), "proactive-source-screen"),
            )
            .changed()
        {
            mind.proactive.sources.screen_summary = screen;
            persist_mind(settings, ai, &mind);
        }
        ui.weak(i18n_embed_fl::fl!(
            crate::i18n::loader(),
            "proactive-source-screen-hint"
        ));
    });
}

/// Window-title capture level combo for the activity source.
///
/// Raising the level lets the proactive observer read the focused window's
/// title; the hint warns that title text is sent to the decision LLM (and to
/// a cloud provider when one is configured).
fn render_window_title_level(
    ui: &mut egui::Ui,
    settings: &mut CharacterSettings,
    ai: &Arc<AiBridge>,
    mind: &mut ene_mind::MindConfig,
) {
    let current = mind.proactive.sources.window_title_level;
    ui.horizontal(|ui| {
        ui.label(i18n_embed_fl::fl!(
            crate::i18n::loader(),
            "proactive-window-title-level"
        ));
        let mut selected = current;
        egui::ComboBox::from_id_salt("proactive_window_title_level")
            .selected_text(window_title_level_label(selected))
            .show_ui(ui, |ui| {
                ui.selectable_value(
                    &mut selected,
                    WindowTitleLevel::AppOnly,
                    i18n_embed_fl::fl!(crate::i18n::loader(), "proactive-title-app-only"),
                );
                ui.selectable_value(
                    &mut selected,
                    WindowTitleLevel::RedactedTitle,
                    i18n_embed_fl::fl!(crate::i18n::loader(), "proactive-title-redacted"),
                );
                ui.selectable_value(
                    &mut selected,
                    WindowTitleLevel::FullTitle,
                    i18n_embed_fl::fl!(crate::i18n::loader(), "proactive-title-full"),
                );
            });
        if selected != current {
            mind.proactive.sources.window_title_level = selected;
            persist_mind(settings, ai, mind);
        }
    });
    ui.weak(i18n_embed_fl::fl!(
        crate::i18n::loader(),
        "proactive-window-title-hint"
    ));
}

fn window_title_level_label(level: WindowTitleLevel) -> String {
    match level {
        WindowTitleLevel::AppOnly => {
            i18n_embed_fl::fl!(crate::i18n::loader(), "proactive-title-app-only")
        }
        WindowTitleLevel::RedactedTitle => {
            i18n_embed_fl::fl!(crate::i18n::loader(), "proactive-title-redacted")
        }
        WindowTitleLevel::FullTitle => {
            i18n_embed_fl::fl!(crate::i18n::loader(), "proactive-title-full")
        }
    }
}

fn persist_mind(settings: &mut CharacterSettings, ai: &Arc<AiBridge>, mind: &ene_mind::MindConfig) {
    settings.set_config_section(mind);
    settings.mark_dirty();
    sync_features(settings, ai);
}

fn sync_features(settings: &CharacterSettings, ai: &Arc<AiBridge>) {
    let mind = settings.config_section::<ene_mind::MindConfig>();
    let store = settings.config_section::<ene_store::StoreConfig>();
    let tools = settings.config_section::<PluginConfig>();
    let rag = settings.config_section::<ToolRagConfig>();
    ai.sync_feature_runtime(&mind, &store, &tools, &rag);
}

fn render_tools(ui: &mut egui::Ui, settings: &mut CharacterSettings, ai: &Arc<AiBridge>) {
    ui.label(i18n_embed_fl::fl!(crate::i18n::loader(), "features-tools"));

    let mut tools = settings.config_section::<PluginConfig>();
    let mut rag = settings.config_section::<ToolRagConfig>();

    let mut tools_enabled = tools.enabled;
    if ui
        .checkbox(
            &mut tools_enabled,
            i18n_embed_fl::fl!(crate::i18n::loader(), "enable-tools"),
        )
        .changed()
    {
        tools.enabled = tools_enabled;
        settings.set_config_section(&tools);
        settings.mark_dirty();
        sync_features(settings, ai);
    }

    ui.add_enabled_ui(tools.enabled, |ui| {
        let mut rag_enabled = rag.enabled;
        if ui
            .checkbox(
                &mut rag_enabled,
                i18n_embed_fl::fl!(crate::i18n::loader(), "enable-tool-rag"),
            )
            .changed()
        {
            rag.enabled = rag_enabled;
            settings.set_config_section(&rag);
            settings.mark_dirty();
            sync_features(settings, ai);
        }

        ui.label(i18n_embed_fl::fl!(
            crate::i18n::loader(),
            "features-per-tool"
        ));
        let mut names: Vec<String> = tools.list.keys().cloned().collect();
        for name in DEFAULT_TOOL_NAMES {
            if !names.iter().any(|n| n == *name) {
                names.push((*name).to_string());
            }
        }
        names.sort();

        let mut list_changed = false;
        for name in names {
            let mut enable = tools.list.get(&name).is_none_or(|entry| entry.enable);
            let label = format!(
                "{} ({})",
                i18n_embed_fl::fl!(crate::i18n::loader(), "enable-tool"),
                tool_display_name(&name)
            );
            if ui.checkbox(&mut enable, label).changed() {
                tools.list.entry(name).or_default().enable = enable;
                list_changed = true;
            }
        }
        if list_changed {
            settings.set_config_section(&tools);
            settings.mark_dirty();
            sync_features(settings, ai);
        }
    });
}

fn render_audio(
    ui: &mut egui::Ui,
    settings: &mut CharacterSettings,
    ai: &Arc<AiBridge>,
    world: &mut World,
) {
    ui.label(i18n_embed_fl::fl!(crate::i18n::loader(), "audio"));

    let mut ai_cfg = settings.config_section::<ene_ai::AiConfig>();
    let mut changed = false;
    let mut mic_device_changed = false;

    // Microphone device selection (stored on the desktop section).
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

    ui.horizontal(|ui| {
        ui.label(i18n_embed_fl::fl!(
            crate::i18n::loader(),
            "audio-stt-provider"
        ));
        ui.weak(provider_display(&ai_cfg.stt.provider));
    });
    ui.horizontal(|ui| {
        ui.label(i18n_embed_fl::fl!(
            crate::i18n::loader(),
            "audio-tts-provider"
        ));
        ui.weak(provider_display(&ai_cfg.tts.provider));
    });
    ui.weak(i18n_embed_fl::fl!(
        crate::i18n::loader(),
        "audio-open-voice-settings"
    ));

    if changed {
        settings.set_config_section(&ai_cfg);
        settings.mark_dirty();
    }

    // Propagate audio settings into the shared `AudioState` so the next
    // mic (re)start picks them up. The capture path snapshots
    // `AudioState.config` / `mic_device` at start, so without this the
    // Features page edits would never reach `start_mic_capture`.
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

    // Push the VAD threshold to the runtime actor like the other feature
    // settings do.
    if changed {
        sync_features(settings, ai);
    }
}

fn provider_display(provider: &str) -> String {
    if provider.is_empty() || provider == "none" {
        i18n_embed_fl::fl!(crate::i18n::loader(), "audio-provider-none")
    } else {
        provider.to_string()
    }
}

fn tool_display_name(name: &str) -> &str {
    match name {
        "fs" => "Filesystem",
        "web" => "Web",
        "browser" => "Browser",
        "utility" => "Utility",
        "app" => "App",
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_tool_names_cover_builtin_set() {
        let defaults = PluginConfig::default();
        for name in DEFAULT_TOOL_NAMES {
            assert!(
                defaults.list.contains_key(*name),
                "missing default tool `{name}`"
            );
        }
    }

    #[test]
    fn tool_display_name_covers_builtins() {
        assert_eq!(tool_display_name("fs"), "Filesystem");
        assert_eq!(tool_display_name("unknown"), "unknown");
    }
}
