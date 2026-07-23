//! Features settings page — mind / tools toggles and proactive policy knobs.
//!
//! Provider / embedding settings stay on the AI tab. This page owns the
//! public-schema switches and proactive timing / source policy.

use crate::ai_bridge::AiBridge;
use crate::settings::CharacterSettings;
use ene_tool_host::ToolConfig;
use ene_tool_rag::ToolRagConfig;
use std::sync::Arc;

/// Known tool binary names shown even when absent from the saved map.
const DEFAULT_TOOL_NAMES: &[&str] = &["app", "browser", "fs", "utility", "web"];

pub fn render(ui: &mut egui::Ui, settings: &mut CharacterSettings, ai: &Arc<AiBridge>) {
    ui.vertical(|ui| {
        ui.weak(crate::i18n::features_hint());
        ui.separator();

        render_mind(ui, settings, ai);
        ui.separator();
        render_tools(ui, settings, ai);
        ui.separator();
        render_audio(ui, settings);
    });
}

fn render_mind(ui: &mut egui::Ui, settings: &mut CharacterSettings, ai: &Arc<AiBridge>) {
    ui.label(crate::i18n::features_mind());

    let mut memory = settings
        .ai
        .ai
        .get_section::<ene_store::StoreConfig>()
        .unwrap_or_default();
    let mut mind = settings
        .ai
        .ai
        .get_section::<ene_mind::MindConfig>()
        .unwrap_or_default();

    let mut memory_enabled = memory.enabled;
    if ui
        .checkbox(&mut memory_enabled, crate::i18n::enable_long_term_memory())
        .changed()
    {
        memory.enabled = memory_enabled;
        let _ = settings.ai.ai.set_section(&memory);
        settings.mark_dirty();
        sync_features(settings, ai);
    }

    let mut emotion_enabled = mind.emotion.enabled;
    if ui
        .checkbox(&mut emotion_enabled, crate::i18n::enable_emotion())
        .changed()
    {
        mind.emotion.enabled = emotion_enabled;
        persist_mind(settings, ai, &mind);
    }

    ui.separator();
    ui.label(crate::i18n::proactive_speech());

    let mut proactive_enabled = mind.proactive.enabled;
    if ui
        .checkbox(&mut proactive_enabled, crate::i18n::proactive_enabled())
        .changed()
    {
        mind.proactive.enabled = proactive_enabled;
        persist_mind(settings, ai, &mind);
    }

    ui.add_enabled_ui(mind.proactive.enabled, |ui| {
        ui.horizontal(|ui| {
            ui.label(crate::i18n::proactive_interval());
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
            ui.label(crate::i18n::proactive_cooldown());
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
            ui.label(crate::i18n::proactive_min_idle());
            let mut value = mind.proactive.min_idle_seconds as i32;
            if ui
                .add(egui::DragValue::new(&mut value).range(0..=86_400))
                .changed()
            {
                mind.proactive.min_idle_seconds = value.max(0) as u64;
                persist_mind(settings, ai, &mind);
            }
        });

        ui.label(crate::i18n::proactive_sources());

        let mut conversation = mind.proactive.sources.conversation;
        if ui
            .checkbox(
                &mut conversation,
                crate::i18n::proactive_source_conversation(),
            )
            .changed()
        {
            mind.proactive.sources.conversation = conversation;
            persist_mind(settings, ai, &mind);
        }

        let mut activity = mind.proactive.sources.activity;
        if ui
            .checkbox(&mut activity, crate::i18n::proactive_source_activity())
            .changed()
        {
            mind.proactive.sources.activity = activity;
            persist_mind(settings, ai, &mind);
        }

        let mut screen = mind.proactive.sources.screen_summary;
        if ui
            .checkbox(&mut screen, crate::i18n::proactive_source_screen())
            .changed()
        {
            mind.proactive.sources.screen_summary = screen;
            persist_mind(settings, ai, &mind);
        }
        ui.weak(crate::i18n::proactive_source_screen_hint());
    });
}

fn persist_mind(settings: &mut CharacterSettings, ai: &Arc<AiBridge>, mind: &ene_mind::MindConfig) {
    let _ = settings.ai.ai.set_section(mind);
    settings.mark_dirty();
    sync_features(settings, ai);
}

fn sync_features(settings: &CharacterSettings, ai: &Arc<AiBridge>) {
    let mind = settings
        .ai
        .ai
        .get_section::<ene_mind::MindConfig>()
        .unwrap_or_default();
    let store = settings
        .ai
        .ai
        .get_section::<ene_store::StoreConfig>()
        .unwrap_or_default();
    let tools = settings
        .ai
        .ai
        .get_section::<ToolConfig>()
        .unwrap_or_default();
    let rag = settings
        .ai
        .ai
        .get_section::<ToolRagConfig>()
        .unwrap_or_default();
    ai.sync_feature_runtime(&mind, &store, &tools, &rag);
}

fn render_tools(ui: &mut egui::Ui, settings: &mut CharacterSettings, ai: &Arc<AiBridge>) {
    ui.label(crate::i18n::features_tools());

    let mut tools = settings
        .ai
        .ai
        .get_section::<ToolConfig>()
        .unwrap_or_default();
    let mut rag = settings
        .ai
        .ai
        .get_section::<ToolRagConfig>()
        .unwrap_or_default();

    let mut tools_enabled = tools.enabled;
    if ui
        .checkbox(&mut tools_enabled, crate::i18n::enable_tools())
        .changed()
    {
        tools.enabled = tools_enabled;
        persist_tools(settings, ai, &tools);
    }

    ui.add_enabled_ui(tools.enabled, |ui| {
        let mut rag_enabled = rag.enabled;
        if ui
            .checkbox(&mut rag_enabled, crate::i18n::enable_tool_rag())
            .changed()
        {
            rag.enabled = rag_enabled;
            let _ = settings.ai.ai.set_section(&rag);
            settings.mark_dirty();
            sync_features(settings, ai);
        }

        ui.label(crate::i18n::features_per_tool());
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
                crate::i18n::enable_tool(),
                tool_display_name(&name)
            );
            if ui.checkbox(&mut enable, label).changed() {
                tools.list.entry(name).or_default().enable = enable;
                list_changed = true;
            }
        }
        if list_changed {
            persist_tools(settings, ai, &tools);
        }
    });
}

/// `ToolConfig` serializes at `tools` and would wipe sibling `tools.rag`.
fn persist_tools(settings: &mut CharacterSettings, ai: &Arc<AiBridge>, tools: &ToolConfig) {
    let rag = settings
        .ai
        .ai
        .get_section::<ToolRagConfig>()
        .unwrap_or_default();
    let _ = settings.ai.ai.set_section(tools);
    let _ = settings.ai.ai.set_section(&rag);
    settings.mark_dirty();
    sync_features(settings, ai);
}

fn render_audio(ui: &mut egui::Ui, settings: &mut CharacterSettings) {
    ui.label(crate::i18n::audio());

    let mut ai_cfg = settings
        .ai
        .ai
        .get_section::<ene_ai::AiConfig>()
        .unwrap_or_default();
    let mut changed = false;

    // Microphone device selection (stored on the desktop section).
    ui.horizontal(|ui| {
        ui.label(crate::i18n::audio_mic_device());
        let mut device = settings.mic_device.clone().unwrap_or_default();
        if ui
            .add(egui::TextEdit::singleline(&mut device).desired_width(200.0))
            .on_hover_text(crate::i18n::audio_mic_default())
            .changed()
        {
            settings.mic_device = if device.trim().is_empty() {
                None
            } else {
                Some(device.trim().to_string())
            };
            settings.mark_dirty();
        }
    });

    // VAD threshold slider.
    ui.horizontal(|ui| {
        ui.label(crate::i18n::audio_vad_threshold());
        let mut threshold = ai_cfg.vad.threshold;
        if ui
            .add(egui::Slider::new(&mut threshold, 0.0..=1.0))
            .changed()
        {
            ai_cfg.vad.threshold = threshold;
            changed = true;
        }
    });

    // Read-only provider info.
    ui.horizontal(|ui| {
        ui.label(crate::i18n::audio_stt_provider());
        ui.weak(provider_display(&ai_cfg.stt.provider));
    });
    ui.horizontal(|ui| {
        ui.label(crate::i18n::audio_tts_provider());
        ui.weak(provider_display(&ai_cfg.tts.provider));
    });

    if changed {
        let _ = settings.ai.ai.set_section(&ai_cfg);
        settings.mark_dirty();
    }
}

fn provider_display(provider: &str) -> String {
    if provider.is_empty() || provider == "none" {
        crate::i18n::audio_provider_none()
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
        let defaults = ToolConfig::default();
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
