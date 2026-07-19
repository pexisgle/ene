//! Features settings page — master on/off toggles for mind and tools.
//!
//! Timing / provider knobs stay on the AI tab; this page only flips
//! the public-schema boolean switches in `settings.json`.

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
        render_tools(ui, settings);
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
    }

    let mut emotion_enabled = mind.emotion.enabled;
    if ui
        .checkbox(&mut emotion_enabled, crate::i18n::enable_emotion())
        .changed()
    {
        mind.emotion.enabled = emotion_enabled;
        let _ = settings.ai.ai.set_section(&mind);
        settings.mark_dirty();
    }

    let mut proactive_enabled = mind.proactive.enabled;
    if ui
        .checkbox(&mut proactive_enabled, crate::i18n::proactive_enabled())
        .changed()
    {
        mind.proactive.enabled = proactive_enabled;
        let _ = settings.ai.ai.set_section(&mind);
        settings.mark_dirty();
        ai.sync_proactive_runtime(&mind);
    }
}

fn render_tools(ui: &mut egui::Ui, settings: &mut CharacterSettings) {
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
        persist_tools(settings, &tools);
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
            persist_tools(settings, &tools);
        }
    });
}

/// `ToolConfig` serializes at `tools` and would wipe sibling `tools.rag`.
fn persist_tools(settings: &mut CharacterSettings, tools: &ToolConfig) {
    let rag = settings
        .ai
        .ai
        .get_section::<ToolRagConfig>()
        .unwrap_or_default();
    let _ = settings.ai.ai.set_section(tools);
    let _ = settings.ai.ai.set_section(&rag);
    settings.mark_dirty();
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
    fn tool_entry_defaults_enabled() {
        assert!(ene_tool_host::ToolEntry::default().enable);
    }
}
