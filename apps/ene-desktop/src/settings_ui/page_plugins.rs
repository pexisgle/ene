//! Tools & Plugins page: tool activation and per-plugin settings in one
//! place.
//!
//! A "General" section holds the master switches and runtime limits, then
//! Tools / Providers / MCP tabs list the configured `plugins.list` entries.
//! Opening a tool or provider shows a kind-specific detail view: actions,
//! a schema-driven config form (with dynamic options bound to
//! `x-ene-ui.options_path` fields), profile editing, plugin validation
//! against the *draft* value, and a Security section grouping sandbox /
//! fs grants / quotas / credentials / approval overrides. Everything lands
//! on the [`SettingsDraft`]; the window-level Apply bar pushes it through
//! validation and the runtime apply pipeline.

use crate::ai_bridge::AiBridge;
use crate::component::ui::UiStateComponent;
use crate::settings::{CharacterSettings, PluginPageMode};
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;

use super::components::{self, BadgeTone, status_badge};
use super::draft::{FieldImpact, SettingsDraft};
use super::input::{AsyncData, SettingsInputState};
use super::schema_form::{SchemaFormOptions, profiles_schema, schema_object_form};
use ene_plugin_host::{PluginHealthState, PluginSettingsSnapshot};
use serde_json::json;
use std::collections::BTreeMap;
use std::sync::Arc;

/// Home Assistant ships a built-in `sun.sun` entity; probing it verifies
/// base URL + token without requiring a user-chosen entity.
const HOMEASSISTANT_PROBE_ENTITY: &str = "sun.sun";

/// Built-in tool plugin ids; a stopped entry advertises no capabilities, so
/// its snapshot kind is "unknown" until the plugin runs.
const KNOWN_TOOL_IDS: &[&str] = &[
    "app",
    "browser",
    "calc",
    "calendar",
    "counter",
    "fs",
    "geo",
    "git",
    "homeassistant",
    "random",
    "utility",
    "web",
];

/// Built-in provider plugin ids; same purpose as [`KNOWN_TOOL_IDS`].
const KNOWN_PROVIDER_IDS: &[&str] = &[
    "anthropic",
    "edge-tts",
    "elevenlabs",
    "kokoro",
    "llama-cpp",
    "llama-server",
    "local-llm",
    "onnx",
    "openai",
    "openai-tts",
    "voicevox",
    "whisper",
];

/// The plugin's kind, falling back to the built-in id tables while the
/// entry is stopped (no connection, so capabilities are not advertised).
fn effective_kind(snapshot: &PluginSettingsSnapshot) -> &str {
    if snapshot.kind != "unknown" {
        return &snapshot.kind;
    }
    if KNOWN_TOOL_IDS.contains(&snapshot.id.as_str()) {
        "tool"
    } else if KNOWN_PROVIDER_IDS.contains(&snapshot.id.as_str()) {
        "provider"
    } else {
        "unknown"
    }
}

fn is_tool_kind(kind: &str) -> bool {
    matches!(kind, "tool" | "hybrid" | "unknown")
}

fn is_provider_kind(kind: &str) -> bool {
    matches!(kind, "provider" | "hybrid")
}

/// Renders the plugin center page.
pub fn render(
    ui: &mut egui::Ui,
    _settings: &mut CharacterSettings,
    draft: &mut SettingsDraft,
    ai: &Arc<AiBridge>,
    input: &mut SettingsInputState,
    world: &mut World,
    ui_entity: Entity,
    plugin_focus: Option<&str>,
) {
    ui.horizontal(|ui| {
        if ui
            .small_button(i18n_embed_fl::fl!(crate::i18n::loader(), "plugins-refresh"))
            .clicked()
        {
            input.plugin_snapshots = AsyncData::new();
            input.mcp_statuses = AsyncData::new();
        }
    });
    ui.weak(i18n_embed_fl::fl!(
        crate::i18n::loader(),
        "plugins-apply-hint"
    ));
    ui.add_space(6.0);

    input.plugin_snapshots.poll();
    if !input.plugin_snapshots.started() {
        input.plugin_snapshots.start(ai.fetch_plugin_snapshots());
    }
    if input.plugin_snapshots.loading() {
        ui.weak(i18n_embed_fl::fl!(crate::i18n::loader(), "plugins-loading"));
    }
    if let Some(error) = input.plugin_snapshots.error.clone() {
        ui.colored_label(egui::Color32::LIGHT_RED, error);
        if ui
            .small_button(i18n_embed_fl::fl!(crate::i18n::loader(), "plugins-retry"))
            .clicked()
        {
            input.plugin_snapshots = AsyncData::new();
        }
    }

    let mut snapshots = input.plugin_snapshots.data.clone().unwrap_or_default();
    // Entries present in the draft but not yet in the runtime config (e.g.
    // just added from the discovered list) get placeholder snapshots so the
    // add is visible before Apply starts the plugin.
    let draft_plugins = draft.section::<ene_plugin_host::PluginConfig>();
    for (name, entry) in &draft_plugins.list {
        if !snapshots.iter().any(|snapshot| &snapshot.id == name) {
            snapshots.push(placeholder_snapshot(name, entry));
        }
    }
    snapshots.sort_by(|a, b| a.id.cmp(&b.id));

    // One-shot search navigation: open the detail view on the tab that
    // owns the focused plugin before the tab content renders.
    if let Some(focus) = plugin_focus {
        apply_plugin_focus(world, ui_entity, focus);
        components::request_section_focus(ui.ctx(), "plugins-detail");
    }

    let mut mode = plugin_mode(world, ui_entity);
    let selected = {
        let current = plugin_selected(world, ui_entity);
        // Only drop the selection once a snapshot fetch has completed; a
        // search hit can land before the page's first fetch finishes.
        if input.plugin_snapshots.data.is_some()
            && current
                .as_ref()
                .is_some_and(|id| !snapshots.iter().any(|s| &s.id == id))
        {
            clear_plugin_selected(world, ui_entity);
            None
        } else {
            current
        }
    };
    // Keep the active tab consistent with the selected plugin's kind, so a
    // search focus whose snapshots were not loaded yet lands on the right
    // tab as soon as they arrive. The MCP tab has no detail view, so any
    // stale selection is dropped instead.
    match mode {
        PluginPageMode::Mcp => {
            if selected.is_some() {
                clear_plugin_selected(world, ui_entity);
            }
        }
        PluginPageMode::Tools | PluginPageMode::Providers => {
            if let Some(id) = selected.as_deref()
                && let Some(snapshot) = snapshots.iter().find(|snapshot| snapshot.id == id)
            {
                let expected = match effective_kind(snapshot) {
                    "provider" => PluginPageMode::Providers,
                    _ => PluginPageMode::Tools,
                };
                if mode != expected {
                    if let Some(mut state) = world.get_mut::<UiStateComponent>(ui_entity) {
                        state.0.plugin_page_mode = expected;
                    }
                    mode = expected;
                }
            }
        }
    }

    render_general(ui, draft);
    render_tabs(ui, world, ui_entity, mode);

    let mut removed: Option<String> = None;
    match mode {
        PluginPageMode::Tools => {
            let tools: Vec<&PluginSettingsSnapshot> = snapshots
                .iter()
                .filter(|snapshot| is_tool_kind(effective_kind(snapshot)))
                .collect();
            render_plugin_section(
                ui,
                draft,
                ai,
                input,
                world,
                ui_entity,
                &tools,
                "plugins-tools",
                "plugins-tab-tools",
                false,
                selected.as_deref(),
                &mut removed,
            );
            render_discovered(ui, draft, ai, input);
        }
        PluginPageMode::Providers => {
            let providers: Vec<&PluginSettingsSnapshot> = snapshots
                .iter()
                .filter(|snapshot| is_provider_kind(effective_kind(snapshot)))
                .collect();
            render_plugin_section(
                ui,
                draft,
                ai,
                input,
                world,
                ui_entity,
                &providers,
                "plugins-providers",
                "plugins-tab-providers",
                true,
                selected.as_deref(),
                &mut removed,
            );
        }
        PluginPageMode::Mcp => render_mcp_section(ui, draft, ai, input),
    }
    if let Some(name) = removed {
        let mut plugins = draft.section::<ene_plugin_host::PluginConfig>();
        plugins.list.remove(&name);
        draft.set_section(&plugins);
        input.plugin_snapshots = AsyncData::new();
        clear_plugin_selected(world, ui_entity);
    }
}

/// Master switches and runtime limits shared by every tab.
fn render_general(ui: &mut egui::Ui, draft: &mut SettingsDraft) {
    let mut plugins = draft.section::<ene_plugin_host::PluginConfig>();
    let mut rag = draft.section::<ene_rag::ToolRagConfig>();
    let mut plugins_changed = false;
    let mut rag_changed = false;
    components::section_card(
        ui,
        "plugins-general",
        &i18n_embed_fl::fl!(crate::i18n::loader(), "plugins-general-title"),
        |ui| {
            if toggle_row(
                ui,
                &i18n_embed_fl::fl!(crate::i18n::loader(), "plugins-master-enable"),
                &i18n_embed_fl::fl!(crate::i18n::loader(), "plugins-master-enable-hint"),
                &mut plugins.enabled,
            ) {
                plugins_changed = true;
            }
            if toggle_row(
                ui,
                &i18n_embed_fl::fl!(crate::i18n::loader(), "enable-tool-rag"),
                "",
                &mut rag.enabled,
            ) {
                rag_changed = true;
            }

            ui.add_space(6.0);
            ui.label(i18n_embed_fl::fl!(
                crate::i18n::loader(),
                "plugins-runtime-title"
            ));
            ui.horizontal(|ui| {
                let mut max_concurrent = plugins.max_concurrent;
                ui.label(i18n_embed_fl::fl!(
                    crate::i18n::loader(),
                    "plugins-max-concurrent"
                ));
                if ui
                    .add(egui::DragValue::new(&mut max_concurrent).range(1..=64))
                    .changed()
                {
                    plugins.max_concurrent = max_concurrent;
                    plugins_changed = true;
                }
            });
            ui.horizontal(|ui| {
                let mut max_rounds = plugins.max_rounds;
                ui.label(i18n_embed_fl::fl!(
                    crate::i18n::loader(),
                    "plugins-max-rounds"
                ));
                if ui
                    .add(egui::DragValue::new(&mut max_rounds).range(1..=64))
                    .changed()
                {
                    plugins.max_rounds = max_rounds;
                    plugins_changed = true;
                }
            });
            ui.horizontal(|ui| {
                let mut timeout_ms = plugins.timeout_ms;
                ui.label(i18n_embed_fl::fl!(
                    crate::i18n::loader(),
                    "plugins-timeout-ms"
                ));
                if ui
                    .add(egui::DragValue::new(&mut timeout_ms).range(100..=3_600_000))
                    .changed()
                {
                    plugins.timeout_ms = timeout_ms;
                    plugins_changed = true;
                }
            });
        },
    );
    if plugins_changed {
        draft.set_section(&plugins);
    }
    if rag_changed {
        draft.set_section(&rag);
    }
}

fn render_tabs(ui: &mut egui::Ui, world: &mut World, ui_entity: Entity, mode: PluginPageMode) {
    let mut next: Option<PluginPageMode> = None;
    ui.horizontal(|ui| {
        for (tab, key) in [
            (PluginPageMode::Tools, "plugins-tab-tools"),
            (PluginPageMode::Providers, "plugins-tab-providers"),
            (PluginPageMode::Mcp, "plugins-tab-mcp"),
        ] {
            if ui
                .selectable_label(mode == tab, crate::i18n::loader().get(key))
                .clicked()
            {
                next = Some(tab);
            }
        }
    });
    if let Some(tab) = next
        && let Some(mut state) = world.get_mut::<UiStateComponent>(ui_entity)
    {
        if state.0.plugin_page_mode != tab {
            state.0.plugin_selected = None;
        }
        state.0.plugin_page_mode = tab;
    }
}

/// List or detail view for one tab: the selected plugin renders its
/// kind-specific detail, everything else renders the row list.
fn render_plugin_section(
    ui: &mut egui::Ui,
    draft: &mut SettingsDraft,
    ai: &Arc<AiBridge>,
    input: &mut SettingsInputState,
    world: &mut World,
    ui_entity: Entity,
    filtered: &[&PluginSettingsSnapshot],
    list_section: &str,
    tab_title_key: &str,
    provider_tab: bool,
    selected: Option<&str>,
    removed: &mut Option<String>,
) {
    let Some(snapshot) = selected.and_then(|id| filtered.iter().find(|snapshot| snapshot.id == id))
    else {
        components::section_card(
            ui,
            list_section,
            &crate::i18n::loader().get(tab_title_key),
            |ui| render_plugin_list(ui, draft, world, ui_entity, filtered, selected),
        );
        return;
    };
    components::section_card(ui, "plugins-detail", &plugin_row_label(snapshot), |ui| {
        *removed = render_plugin_detail(
            ui,
            draft,
            ai,
            input,
            world,
            ui_entity,
            snapshot,
            provider_tab,
        );
    });
}

fn render_plugin_list(
    ui: &mut egui::Ui,
    draft: &mut SettingsDraft,
    world: &mut World,
    ui_entity: Entity,
    filtered: &[&PluginSettingsSnapshot],
    selected: Option<&str>,
) {
    if filtered.is_empty() {
        ui.weak(i18n_embed_fl::fl!(crate::i18n::loader(), "plugins-none"));
        return;
    }
    for snapshot in filtered {
        let enabled_key = format!("plugins.list.{}.enable", snapshot.id);
        let mut enabled = draft
            .editing()
            .get_path(&enabled_key)
            .and_then(|v| v.as_bool())
            .unwrap_or(snapshot.enabled);
        let id = snapshot.id.clone();
        ui.horizontal_wrapped(|ui| {
            if ui.checkbox(&mut enabled, "").changed() {
                draft.set_path(&enabled_key, serde_json::Value::Bool(enabled));
            }
            if ui
                .selectable_label(selected == Some(id.as_str()), plugin_row_label(snapshot))
                .clicked()
            {
                select_plugin(world, ui_entity, id.clone());
            }
            render_health_badge(ui, snapshot);
            if !snapshot.actions.is_empty() {
                ui.weak(i18n_embed_fl::fl!(
                    crate::i18n::loader(),
                    "plugins-actions-count",
                    count = snapshot.actions.len()
                ));
            }
        });
    }
}

fn render_plugin_detail(
    ui: &mut egui::Ui,
    draft: &mut SettingsDraft,
    ai: &Arc<AiBridge>,
    input: &mut SettingsInputState,
    world: &mut World,
    ui_entity: Entity,
    snapshot: &PluginSettingsSnapshot,
    provider_tab: bool,
) -> Option<String> {
    let mut removed = None;
    ui.horizontal(|ui| {
        if ui
            .small_button(i18n_embed_fl::fl!(
                crate::i18n::loader(),
                "plugins-back-to-list"
            ))
            .clicked()
        {
            clear_plugin_selected(world, ui_entity);
        }
    });
    ui.add_space(4.0);
    ui.horizontal_wrapped(|ui| {
        status_badge(ui, &plugin_kind_label(snapshot), BadgeTone::Neutral);
        render_health_badge(ui, snapshot);
    });
    ui.horizontal_wrapped(|ui| {
        ui.label(i18n_embed_fl::fl!(crate::i18n::loader(), "plugins-enabled"));
        let enabled_key = format!("plugins.list.{}.enable", snapshot.id);
        let mut enabled = draft
            .editing()
            .get_path(&enabled_key)
            .and_then(|v| v.as_bool())
            .unwrap_or(snapshot.enabled);
        if ui.checkbox(&mut enabled, "").changed() {
            draft.set_path(&enabled_key, serde_json::Value::Bool(enabled));
        }
        ui.weak(format!(
            "{}: {}",
            i18n_embed_fl::fl!(crate::i18n::loader(), "plugins-impact"),
            if enabled {
                FieldImpact::PluginRestart.code()
            } else {
                FieldImpact::Immediate.code()
            }
        ));
        if ui
            .small_button(i18n_embed_fl::fl!(crate::i18n::loader(), "plugins-remove"))
            .on_hover_text(i18n_embed_fl::fl!(
                crate::i18n::loader(),
                "plugins-remove-hint"
            ))
            .clicked()
        {
            removed = Some(snapshot.id.clone());
        }
    });
    ui.add_space(4.0);

    render_capabilities(ui, snapshot);
    render_manifest(ui, snapshot);
    render_builtin_hints(ui, ai, input, snapshot);

    if provider_tab {
        render_profiles(ui, draft, snapshot);
        render_config(ui, ai, draft, input, snapshot);
        render_credentials_editor(ui, draft, snapshot);
        render_security(ui, draft, snapshot, false);
    } else {
        render_action_list(ui, snapshot);
        render_config(ui, ai, draft, input, snapshot);
        render_profiles(ui, draft, snapshot);
        render_security(ui, draft, snapshot, true);
    }
    removed
}

fn render_health_badge(ui: &mut egui::Ui, snapshot: &PluginSettingsSnapshot) {
    let (label, tone) = match snapshot.health {
        PluginHealthState::Running => (
            i18n_embed_fl::fl!(crate::i18n::loader(), "plugins-health-running"),
            BadgeTone::Ok,
        ),
        PluginHealthState::Disabled => (
            i18n_embed_fl::fl!(crate::i18n::loader(), "plugins-health-disabled"),
            BadgeTone::Error,
        ),
        PluginHealthState::RequirementsUnmet => (
            i18n_embed_fl::fl!(crate::i18n::loader(), "plugins-health-requirements_unmet"),
            BadgeTone::Warn,
        ),
        PluginHealthState::Stopped => (
            i18n_embed_fl::fl!(crate::i18n::loader(), "plugins-health-stopped"),
            BadgeTone::Neutral,
        ),
    };
    status_badge(ui, &label, tone);
}

fn plugin_kind_label(snapshot: &PluginSettingsSnapshot) -> String {
    match effective_kind(snapshot) {
        "tool" => i18n_embed_fl::fl!(crate::i18n::loader(), "plugins-kind-tool"),
        "provider" => i18n_embed_fl::fl!(crate::i18n::loader(), "plugins-kind-provider"),
        "hybrid" => i18n_embed_fl::fl!(crate::i18n::loader(), "plugins-kind-hybrid"),
        _ => i18n_embed_fl::fl!(crate::i18n::loader(), "plugins-kind-unknown"),
    }
}

fn plugin_row_label(snapshot: &PluginSettingsSnapshot) -> String {
    let display = plugin_display_name(snapshot);
    if display == snapshot.id {
        snapshot.id.clone()
    } else {
        format!("{display} ({})", snapshot.id)
    }
}

fn plugin_display_name(snapshot: &PluginSettingsSnapshot) -> String {
    match effective_kind(snapshot) {
        "tool" | "hybrid" => tool_display_name(&snapshot.id).to_string(),
        _ => snapshot.id.clone(),
    }
}

/// Minimal snapshot for a draft-only `plugins.list` entry: the runtime has
/// not started it yet, so there is no connection, schema, or capability
/// data to show.
fn placeholder_snapshot(id: &str, entry: &ene_plugin_host::PluginEntry) -> PluginSettingsSnapshot {
    let kind = if KNOWN_TOOL_IDS.contains(&id) {
        "tool"
    } else if KNOWN_PROVIDER_IDS.contains(&id) {
        "provider"
    } else {
        "unknown"
    };
    PluginSettingsSnapshot {
        id: id.to_string(),
        kind: kind.to_string(),
        enabled: entry.enable,
        health: PluginHealthState::Stopped,
        capabilities: ene_plugin_proto::PluginCapabilities::default(),
        actions: Vec::new(),
        manifest: ene_plugin_host::PluginManifestSummary {
            key_id: None,
            signed: false,
            checksum: None,
        },
        schema: None,
        schema_version: 0,
        supports_dynamic_config: false,
        supports_validate_config: false,
        config: None,
        profiles: None,
        credentials: Vec::new(),
        effective_security: ene_plugin_host::EffectiveSecurity::default(),
    }
}

fn tool_display_name(name: &str) -> &str {
    match name {
        "app" => "App",
        "browser" => "Browser",
        "calc" => "Calculator",
        "calendar" => "Calendar",
        "fs" => "Filesystem",
        "random" => "Random",
        "geo" => "Geo",
        "git" => "Git",
        "homeassistant" => "Home Assistant",
        "utility" => "Utility",
        "web" => "Web",
        other => other,
    }
}

fn plugin_mode(world: &World, ui_entity: Entity) -> PluginPageMode {
    world
        .get::<UiStateComponent>(ui_entity)
        .map_or(PluginPageMode::default(), |state| state.0.plugin_page_mode)
}

fn plugin_selected(world: &World, ui_entity: Entity) -> Option<String> {
    world
        .get::<UiStateComponent>(ui_entity)
        .and_then(|state| state.0.plugin_selected.clone())
}

fn select_plugin(world: &mut World, ui_entity: Entity, id: String) {
    if let Some(mut state) = world.get_mut::<UiStateComponent>(ui_entity) {
        state.0.plugin_selected = Some(id);
    }
}

fn clear_plugin_selected(world: &mut World, ui_entity: Entity) {
    if let Some(mut state) = world.get_mut::<UiStateComponent>(ui_entity) {
        state.0.plugin_selected = None;
    }
}

fn apply_plugin_focus(world: &mut World, ui_entity: Entity, focus: &str) {
    if let Some(mut state) = world.get_mut::<UiStateComponent>(ui_entity) {
        state.0.plugin_selected = Some(focus.to_string());
        if state.0.plugin_page_mode == PluginPageMode::Mcp {
            state.0.plugin_page_mode = PluginPageMode::Tools;
        }
    }
}

/// Schema-driven config form with draft validation; falls back to the raw
/// JSON editor when the plugin advertises no schema.
fn render_config(
    ui: &mut egui::Ui,
    ai: &Arc<AiBridge>,
    draft: &mut SettingsDraft,
    input: &mut SettingsInputState,
    snapshot: &PluginSettingsSnapshot,
) {
    let schema_path = format!("plugins.list.{}.config", snapshot.id);
    let mut config_value = draft
        .editing()
        .get_path(&schema_path)
        .unwrap_or_else(|| serde_json::Value::Object(serde_json::Map::new()));
    if let Some(schema) = &snapshot.schema {
        let options = fetch_options(ui, ai, input, snapshot, schema);
        egui::CollapsingHeader::new(i18n_embed_fl::fl!(crate::i18n::loader(), "plugins-config"))
            .id_salt(("plugin_config", snapshot.id.as_str()))
            .default_open(
                !config_value
                    .as_object()
                    .is_none_or(serde_json::Map::is_empty),
            )
            .show(ui, |ui| {
                if schema_object_form(
                    ui,
                    schema,
                    &mut config_value,
                    &schema_path,
                    SchemaFormOptions {
                        show_advanced: true,
                        show_impact: true,
                        epoch: draft.applied_revision(),
                        options: Some(&options),
                    },
                ) {
                    draft.set_path(&schema_path, config_value);
                }
                render_draft_validation(ui, ai, draft, input, snapshot);
            });
    } else if config_value
        .as_object()
        .is_none_or(serde_json::Map::is_empty)
    {
        ui.weak(i18n_embed_fl::fl!(
            crate::i18n::loader(),
            "plugins-no-config-needed"
        ));
    } else {
        egui::CollapsingHeader::new(i18n_embed_fl::fl!(crate::i18n::loader(), "plugins-config"))
            .id_salt(("plugin_config_raw", snapshot.id.as_str()))
            .show(ui, |ui| {
                if super::schema_form::raw_json_form(
                    ui,
                    &mut config_value,
                    &schema_path,
                    &serde_json::Value::Null,
                    draft.applied_revision(),
                ) {
                    draft.set_path(&schema_path, config_value);
                }
            });
    }
}

/// Security grouping: effective posture, entry settings, and per-plugin
/// approvals. Credentials join the group for tools; providers keep them as
/// a standalone section next to their config.
fn render_security(
    ui: &mut egui::Ui,
    draft: &mut SettingsDraft,
    snapshot: &PluginSettingsSnapshot,
    include_credentials: bool,
) {
    egui::CollapsingHeader::new(i18n_embed_fl::fl!(
        crate::i18n::loader(),
        "plugins-security"
    ))
    .id_salt(("plugin_security", snapshot.id.as_str()))
    .show(ui, |ui| {
        render_effective_security(ui, snapshot);
        render_entry_settings(ui, draft, snapshot);
        if include_credentials {
            render_credentials_editor(ui, draft, snapshot);
        }
        render_approvals_editor(ui, draft, snapshot);
    });
}

/// Runs the plugin's own validator against the *draft* config value (not the
/// snapshot) and shows the result.
fn render_draft_validation(
    ui: &mut egui::Ui,
    ai: &Arc<AiBridge>,
    draft: &SettingsDraft,
    input: &mut SettingsInputState,
    snapshot: &PluginSettingsSnapshot,
) {
    let validate_clicked = ui
        .add_enabled(
            snapshot.supports_validate_config,
            egui::Button::new(i18n_embed_fl::fl!(
                crate::i18n::loader(),
                "plugins-validate"
            )),
        )
        .on_hover_text(i18n_embed_fl::fl!(
            crate::i18n::loader(),
            "plugins-validate-hint"
        ))
        .clicked();
    let state = input
        .plugin_validation
        .entry(snapshot.id.clone())
        .or_default();
    state.poll();
    if validate_clicked {
        // Validate the *merged* draft value so placeholders never reach the
        // plugin's own validator.
        let merged = super::apply::merge_secrets(draft.persisted(), draft.editing());
        let draft_value = merged
            .get_path(&format!("plugins.list.{}.config", snapshot.id))
            .unwrap_or_else(|| serde_json::Value::Object(serde_json::Map::new()));
        let plugin = snapshot.id.clone();
        state.start(ai.validate_plugin_config_async(plugin, draft_value));
    }
    if state.loading() {
        ui.weak(i18n_embed_fl::fl!(crate::i18n::loader(), "plugins-loading"));
        return;
    }
    let validation_result = state.data.clone().unwrap_or_default();
    if !validation_result.is_empty() {
        ui.colored_label(
            egui::Color32::from_rgb(0xff, 0x8a, 0x65),
            i18n_embed_fl::fl!(crate::i18n::loader(), "plugins-validation-errors"),
        );
        for error in &validation_result {
            ui.weak(error);
        }
    } else if state.data.is_some() {
        ui.weak(i18n_embed_fl::fl!(
            crate::i18n::loader(),
            "plugins-validation-ok"
        ));
    }
    if let Some(error) = state.error.clone() {
        ui.colored_label(egui::Color32::LIGHT_RED, error);
    }
}

/// Fetches `ListConfigOptions` for every schema field with
/// `x-ene-ui.options_path`, asynchronously and cached per plugin.
fn fetch_options(
    ui: &mut egui::Ui,
    ai: &Arc<AiBridge>,
    input: &mut SettingsInputState,
    snapshot: &PluginSettingsSnapshot,
    schema: &serde_json::Value,
) -> BTreeMap<String, Vec<(String, String)>> {
    let state = input.plugin_options.entry(snapshot.id.clone()).or_default();
    state.poll();
    let mut fields = Vec::new();
    collect_options_paths(schema, "", &mut fields);
    if !state.started() {
        let plugin = snapshot.id.clone();
        state.start(ai.fetch_plugin_options(plugin, fields));
    }
    if state.loading() {
        ui.weak(i18n_embed_fl::fl!(crate::i18n::loader(), "plugins-loading"));
    }
    if let Some(error) = state.error.clone() {
        ui.colored_label(egui::Color32::LIGHT_RED, error);
    }
    if let Some(map) = &state.data
        && !map.is_empty()
    {
        return map.clone();
    }
    BTreeMap::new()
}

fn collect_options_paths(
    schema: &serde_json::Value,
    prefix: &str,
    out: &mut Vec<(String, String)>,
) {
    let meta = super::schema_form::UiMetadata::from_schema(schema);
    if let Some(options_path) = meta.options_path {
        out.push((prefix.to_string(), options_path));
    }
    if let Some(properties) = schema
        .get("properties")
        .and_then(serde_json::Value::as_object)
    {
        for (name, property_schema) in properties {
            let child = if prefix.is_empty() {
                name.clone()
            } else {
                format!("{prefix}.{name}")
            };
            collect_options_paths(property_schema, &child, out);
        }
    }
    if let Some(items) = schema.get("items") {
        collect_options_paths(items, prefix, out);
    }
}

fn render_capabilities(ui: &mut egui::Ui, snapshot: &PluginSettingsSnapshot) {
    let caps = &snapshot.capabilities;
    let mut parts = Vec::new();
    if caps.tools > 0 {
        parts.push(format!("{} tools", caps.tools));
    }
    for provider in &caps.llm_providers {
        parts.push(format!("llm:{}", provider.kind));
    }
    for provider in &caps.embed_providers {
        parts.push(format!("embed:{provider}"));
    }
    for provider in &caps.tts_providers {
        parts.push(format!("tts:{}", provider.kind));
    }
    for provider in &caps.stt_providers {
        parts.push(format!("stt:{}", provider.kind));
    }
    if !caps.vad_providers.is_empty() {
        parts.push(format!("vad:{}", caps.vad_providers.len()));
    }
    if parts.is_empty() {
        parts
            .push(i18n_embed_fl::fl!(crate::i18n::loader(), "plugins-no-capabilities").to_string());
    }
    ui.weak(format!(
        "{}: {}",
        i18n_embed_fl::fl!(crate::i18n::loader(), "plugins-capabilities"),
        parts.join(", ")
    ));
}

fn render_manifest(ui: &mut egui::Ui, snapshot: &PluginSettingsSnapshot) {
    let manifest = &snapshot.manifest;
    let mut facts = Vec::new();
    if manifest.signed {
        facts.push(i18n_embed_fl::fl!(crate::i18n::loader(), "plugins-signed").to_string());
    }
    if let Some(key_id) = &manifest.key_id {
        facts.push(format!("key:{key_id}"));
    }
    if let Some(checksum) = &manifest.checksum {
        let short: String = checksum.chars().take(12).collect();
        facts.push(format!("sha256:{short}…"));
    }
    if snapshot.schema_version > 0 {
        facts.push(format!("schema v{}", snapshot.schema_version));
    }
    if snapshot.supports_dynamic_config {
        facts.push(i18n_embed_fl::fl!(crate::i18n::loader(), "plugins-dynamic-config").to_string());
    }
    if snapshot.supports_validate_config {
        facts.push(
            i18n_embed_fl::fl!(crate::i18n::loader(), "plugins-validates-config").to_string(),
        );
    }
    if !facts.is_empty() {
        ui.weak(format!(
            "{}: {}",
            i18n_embed_fl::fl!(crate::i18n::loader(), "plugins-manifest"),
            facts.join(", ")
        ));
    }
}

/// Family-specific hints for known built-in plugins: profile guidance,
/// account hints, advertised voices/models, and connectivity checks.
fn render_builtin_hints(
    ui: &mut egui::Ui,
    ai: &Arc<AiBridge>,
    input: &mut SettingsInputState,
    snapshot: &PluginSettingsSnapshot,
) {
    match snapshot.id.as_str() {
        "local-llm" | "llama-server" => {
            ui.weak(i18n_embed_fl::fl!(
                crate::i18n::loader(),
                "plugins-local-profiles-hint"
            ));
        }
        "openai" | "anthropic" => {
            ui.weak(i18n_embed_fl::fl!(
                crate::i18n::loader(),
                "plugins-cloud-account-hint"
            ));
        }
        "kokoro" | "edge-tts" | "elevenlabs" | "openai-tts" | "voicevox" => {
            let voices: Vec<&str> = snapshot
                .capabilities
                .tts_providers
                .iter()
                .flat_map(|provider| provider.voices.iter().map(String::as_str))
                .collect();
            if !voices.is_empty() {
                ui.weak(format!(
                    "{}: {}",
                    i18n_embed_fl::fl!(crate::i18n::loader(), "plugins-voices"),
                    voices.join(", ")
                ));
            }
        }
        "whisper" | "onnx" => {
            let models: Vec<&str> = snapshot
                .capabilities
                .stt_providers
                .iter()
                .flat_map(|provider| provider.models.iter().map(String::as_str))
                .collect();
            if !models.is_empty() {
                ui.weak(format!(
                    "{}: {}",
                    i18n_embed_fl::fl!(crate::i18n::loader(), "plugins-stt-models"),
                    models.join(", ")
                ));
            }
        }
        "homeassistant" => render_homeassistant_test(ui, ai, input),
        "calendar" => {
            render_connector_accounts(ui, ai, input);
        }
        "fs" => {
            ui.weak(i18n_embed_fl::fl!(
                crate::i18n::loader(),
                "plugins-fs-grants-hint"
            ));
        }
        _ => {}
    }
}

/// Lists the plugin's tool/action names with their descriptions (searchable
/// and useful for tools without a config schema).
fn render_action_list(ui: &mut egui::Ui, snapshot: &PluginSettingsSnapshot) {
    if snapshot.actions.is_empty() {
        return;
    }
    egui::CollapsingHeader::new(format!(
        "{} ({})",
        i18n_embed_fl::fl!(crate::i18n::loader(), "plugins-actions"),
        snapshot.actions.len()
    ))
    .id_salt(("plugin_actions", snapshot.id.as_str()))
    .show(ui, |ui| {
        for action in &snapshot.actions {
            ui.label(&action.name);
            if !action.description.is_empty() {
                ui.weak(&action.description);
            }
        }
    });
}

/// Connector accounts relevant to a plugin (e.g. the calendar account
/// registry): live data from the connector framework, fetched asynchronously.
fn render_connector_accounts(
    ui: &mut egui::Ui,
    ai: &Arc<AiBridge>,
    input: &mut SettingsInputState,
) {
    input.connectors.poll();
    if !input.connectors.started() {
        // Started lazily by the Connectors page; start here for the plugin
        // card when the page was never opened.
        input.connectors.start(ai.fetch_connectors());
        return;
    }
    if input.connectors.loading() {
        ui.weak(i18n_embed_fl::fl!(crate::i18n::loader(), "plugins-loading"));
        return;
    }
    let summaries = input.connectors.data.clone().unwrap_or_default();
    if summaries.is_empty() {
        ui.weak(i18n_embed_fl::fl!(
            crate::i18n::loader(),
            "plugins-calendar-no-accounts"
        ));
        return;
    }
    for summary in &summaries {
        ui.label(format!(
            "{} — {} ({} {})",
            summary.identity.id,
            summary.identity.display_name,
            summary.account_count,
            i18n_embed_fl::fl!(crate::i18n::loader(), "connectors-accounts-label")
        ));
    }
}

fn render_homeassistant_test(
    ui: &mut egui::Ui,
    ai: &Arc<AiBridge>,
    input: &mut SettingsInputState,
) {
    let clicked = ui
        .button(i18n_embed_fl::fl!(
            crate::i18n::loader(),
            "plugins-homeassistant-test"
        ))
        .clicked();
    let state = input
        .plugin_tool_test
        .entry("homeassistant".to_string())
        .or_default();
    state.poll();
    if clicked {
        let arguments = json!({ "entity_id": HOMEASSISTANT_PROBE_ENTITY }).to_string();
        let ok_label = i18n_embed_fl::fl!(crate::i18n::loader(), "plugins-homeassistant-ok");
        let error_label = i18n_embed_fl::fl!(crate::i18n::loader(), "plugins-homeassistant-failed");
        let receiver = ai.apply_call_tool("homeassistant.state".to_string(), arguments);
        let message = ai.spawn_fetch(async move {
            match receiver.await {
                Ok(Ok(response)) => format!("{ok_label}: {response}"),
                Ok(Err(error)) => format!("{error_label}: {error}"),
                Err(_) => error_label,
            }
        });
        state.start(message);
    }
    if state.loading() {
        ui.weak(i18n_embed_fl::fl!(crate::i18n::loader(), "plugins-loading"));
    }
    if let Some(result) = state.data.clone() {
        ui.weak(result);
    }
}

fn render_profiles(
    ui: &mut egui::Ui,
    draft: &mut SettingsDraft,
    snapshot: &PluginSettingsSnapshot,
) {
    let Some(schema) = &snapshot.schema else {
        return;
    };
    let Some(profile_schema) = profiles_schema(schema) else {
        return;
    };
    let profiles_path = format!("plugins.list.{}.profiles", snapshot.id);
    let mut profiles = draft
        .editing()
        .get_path(&profiles_path)
        .unwrap_or_else(|| serde_json::Value::Object(serde_json::Map::new()));
    let mut changed = false;
    egui::CollapsingHeader::new(i18n_embed_fl::fl!(
        crate::i18n::loader(),
        "plugins-profiles"
    ))
    .id_salt(("plugin_profiles", snapshot.id.as_str()))
    .default_open(snapshot.id == "local-llm" || snapshot.id == "llama-server")
    .show(ui, |ui| {
        let names: Vec<String> = profiles
            .as_object()
            .map(|o| o.keys().cloned().collect())
            .unwrap_or_default();
        let mut remove: Option<String> = None;
        for name in names {
            egui::CollapsingHeader::new(&name)
                .id_salt(("plugin_profile", snapshot.id.as_str(), name.as_str()))
                .show(ui, |ui| {
                    let Some(profile_value) =
                        profiles.as_object_mut().and_then(|o| o.get_mut(&name))
                    else {
                        return;
                    };
                    let profile_path = format!("{profiles_path}.{name}");
                    if schema_object_form(
                        ui,
                        profile_schema,
                        profile_value,
                        &profile_path,
                        SchemaFormOptions {
                            epoch: draft.applied_revision(),
                            ..SchemaFormOptions::default()
                        },
                    ) {
                        changed = true;
                    }
                    if ui
                        .small_button(i18n_embed_fl::fl!(crate::i18n::loader(), "plugins-remove"))
                        .on_hover_text(i18n_embed_fl::fl!(
                            crate::i18n::loader(),
                            "plugins-profile-remove-hint"
                        ))
                        .clicked()
                    {
                        remove = Some(name.clone());
                    }
                });
        }
        if let Some(name) = remove
            && let Some(object) = profiles.as_object_mut()
        {
            object.remove(&name);
            changed = true;
        }
        let add_name_id = egui::Id::new(("plugin_profile_add", snapshot.id.as_str()));
        let mut add_name =
            ui.data_mut(|data| data.get_temp::<String>(add_name_id).unwrap_or_default());
        ui.horizontal(|ui| {
            let response = ui.add(egui::TextEdit::singleline(&mut add_name).desired_width(160.0));
            if response.changed() {
                ui.data_mut(|data| {
                    data.insert_temp(add_name_id, add_name.clone());
                });
            }
            if ui
                .small_button(i18n_embed_fl::fl!(
                    crate::i18n::loader(),
                    "plugins-add-profile"
                ))
                .clicked()
                && !add_name.trim().is_empty()
            {
                let name = add_name.trim().to_string();
                if let Some(object) = profiles.as_object_mut() {
                    object.insert(
                        name.clone(),
                        profile_schema
                            .get("default")
                            .cloned()
                            .unwrap_or_else(|| serde_json::Value::Object(serde_json::Map::new())),
                    );
                    changed = true;
                }
                ui.data_mut(|data| {
                    data.insert_temp(add_name_id, String::new());
                });
            }
        });
    });
    if changed {
        draft.set_path(&profiles_path, profiles);
    }
}

/// Edits the host-owned entry fields (`enable`, `checksum`,
/// `env_passthrough`, `db_quota_mb`, `sandbox`, `fs_grants`) through the
/// `PluginEntry` schema. `config` / `profiles` / `credentials` are edited by
/// their own sections and excluded here so secrets never reach a raw-JSON
/// fallback.
fn render_entry_settings(
    ui: &mut egui::Ui,
    draft: &mut SettingsDraft,
    snapshot: &PluginSettingsSnapshot,
) {
    let Some(mut entry_schema) = plugin_entry_schema() else {
        return;
    };
    if let Some(properties) = entry_schema
        .get_mut("properties")
        .and_then(serde_json::Value::as_object_mut)
    {
        properties.remove("config");
        properties.remove("profiles");
        properties.remove("credentials");
        properties.remove("extra");
    }
    let entry_path = format!("plugins.list.{}", snapshot.id);
    let mut entry_value = draft
        .editing()
        .get_path(&entry_path)
        .unwrap_or_else(|| serde_json::Value::Object(serde_json::Map::new()));
    // The form never sees config/profiles/credentials (they are edited by
    // dedicated sections), so the unknown-key raw fallback cannot expose
    // their secrets either.
    let mut form_value = entry_value.clone();
    if let Some(object) = form_value.as_object_mut() {
        object.remove("config");
        object.remove("profiles");
        object.remove("credentials");
    }
    let mut changed = false;
    egui::CollapsingHeader::new(i18n_embed_fl::fl!(
        crate::i18n::loader(),
        "plugins-entry-settings"
    ))
    .id_salt(("plugin_entry", snapshot.id.as_str()))
    .show(ui, |ui| {
        changed = schema_object_form(
            ui,
            &entry_schema,
            &mut form_value,
            &entry_path,
            SchemaFormOptions {
                show_advanced: true,
                epoch: draft.applied_revision(),
                ..SchemaFormOptions::default()
            },
        );
    });
    if changed {
        if let (Some(entry_object), Some(form_object)) =
            (entry_value.as_object_mut(), form_value.as_object())
        {
            for (key, value) in form_object {
                entry_object.insert(key.clone(), value.clone());
            }
        }
        draft.set_path(&entry_path, entry_value);
    }
}

/// Resolves the `PluginEntry` schema from the registered `plugins` section
/// schema (`$defs.PluginEntry` / `definitions.PluginEntry` / the list's
/// `additionalProperties`).
fn plugin_entry_schema() -> Option<serde_json::Value> {
    let (_, entry) = ene_config::config::registered_schemas_for(ene_config::ConfigTarget::Settings)
        .into_iter()
        .find(|(key, _)| key == "plugins")?;
    let schema = serde_json::to_value(&entry.schema).ok()?;
    for pointer in [
        "/$defs/PluginEntry",
        "/definitions/PluginEntry",
        "/properties/list/additionalProperties",
    ] {
        if let Some(value) = schema.pointer(pointer) {
            if value.get("type").is_some() {
                return Some(value.clone());
            }
            if let Some(reference) = value.get("$ref").and_then(serde_json::Value::as_str) {
                let name = reference
                    .strip_prefix("#/$defs/")
                    .or_else(|| reference.strip_prefix("#/definitions/"))?;
                for defs_pointer in ["/$defs", "/definitions"] {
                    if let Some(resolved) =
                        schema.pointer(defs_pointer).and_then(|defs| defs.get(name))
                    {
                        return Some(resolved.clone());
                    }
                }
            }
            return Some(value.clone());
        }
    }
    None
}

/// Host-owned credential map (`plugins.list.<name>.credentials`): secret
/// values with add/remove.
fn render_credentials_editor(
    ui: &mut egui::Ui,
    draft: &mut SettingsDraft,
    snapshot: &PluginSettingsSnapshot,
) {
    let credentials_path = format!("plugins.list.{}.credentials", snapshot.id);
    let mut credentials = draft
        .editing()
        .get_path(&credentials_path)
        .unwrap_or_else(|| serde_json::Value::Object(serde_json::Map::new()));
    let mut changed = false;
    egui::CollapsingHeader::new(i18n_embed_fl::fl!(
        crate::i18n::loader(),
        "plugins-credentials"
    ))
    .id_salt(("plugin_credentials_edit", snapshot.id.as_str()))
    .show(ui, |ui| {
        let names: Vec<String> = credentials
            .as_object()
            .map(|o| o.keys().cloned().collect())
            .unwrap_or_default();
        let mut remove: Option<String> = None;
        for name in names {
            ui.horizontal(|ui| {
                ui.label(&name);
                let secret_id =
                    egui::Id::new(("plugin_credential", snapshot.id.as_str(), name.as_str()));
                let mut buffer =
                    ui.data_mut(|data| data.get_temp::<String>(secret_id).unwrap_or_default());
                let response = ui.add(
                    egui::TextEdit::singleline(&mut buffer)
                        .password(true)
                        .desired_width(180.0),
                );
                if response.changed() {
                    ui.data_mut(|data| {
                        data.insert_temp(secret_id, buffer.clone());
                    });
                    if !buffer.is_empty()
                        && let Some(object) = credentials.as_object_mut()
                    {
                        object.insert(name.clone(), serde_json::Value::String(buffer.clone()));
                        changed = true;
                    }
                }
                if ui
                    .small_button(i18n_embed_fl::fl!(crate::i18n::loader(), "plugins-remove"))
                    .clicked()
                {
                    remove = Some(name.clone());
                }
            });
        }
        if let Some(name) = remove
            && let Some(object) = credentials.as_object_mut()
        {
            object.remove(&name);
            changed = true;
        }
        let add_key_id = egui::Id::new(("plugin_credential_add", snapshot.id.as_str()));
        let mut add_key =
            ui.data_mut(|data| data.get_temp::<String>(add_key_id).unwrap_or_default());
        ui.horizontal(|ui| {
            let response = ui.add(egui::TextEdit::singleline(&mut add_key).desired_width(160.0));
            if response.changed() {
                ui.data_mut(|data| {
                    data.insert_temp(add_key_id, add_key.clone());
                });
            }
            if ui
                .small_button(i18n_embed_fl::fl!(
                    crate::i18n::loader(),
                    "plugins-add-credential"
                ))
                .clicked()
                && !add_key.trim().is_empty()
            {
                if let Some(object) = credentials.as_object_mut() {
                    object.insert(
                        add_key.trim().to_string(),
                        serde_json::Value::String(String::new()),
                    );
                    changed = true;
                }
                ui.data_mut(|data| {
                    data.insert_temp(add_key_id, String::new());
                });
            }
        });
    });
    if changed {
        draft.set_path(&credentials_path, credentials);
    }
}

/// Per-plugin approval overrides (`plugins.plugin_approval.<name>`).
fn render_approvals_editor(
    ui: &mut egui::Ui,
    draft: &mut SettingsDraft,
    snapshot: &PluginSettingsSnapshot,
) {
    egui::CollapsingHeader::new(i18n_embed_fl::fl!(
        crate::i18n::loader(),
        "plugins-approvals"
    ))
    .id_salt(("plugin_approvals", snapshot.id.as_str()))
    .show(ui, |ui| {
        for category in ene_approval::ALL_CATEGORIES {
            let category_key = serde_json::to_value(category)
                .ok()
                .and_then(|v| v.as_str().map(str::to_string))
                .unwrap_or_default();
            let path = format!(
                "plugins.plugin_approval.{}.categories.{category_key}",
                snapshot.id
            );
            let current = draft
                .editing()
                .get_path(&path)
                .and_then(|v| v.as_str().map(str::to_string))
                .unwrap_or_else(|| "inherit".to_string());
            let mut selected = current.clone();
            ui.horizontal(|ui| {
                ui.label(&category_key);
                egui::ComboBox::from_id_salt((
                    "plugin_approval_combo",
                    snapshot.id.as_str(),
                    category_key.as_str(),
                ))
                .selected_text(selected.as_str())
                .show_ui(ui, |ui| {
                    for mode in ["inherit", "ask", "allow", "deny"] {
                        if ui.selectable_label(selected == mode, mode).clicked() {
                            selected = mode.to_string();
                        }
                    }
                });
            });
            if selected != current {
                draft.set_path(&path, serde_json::Value::String(selected));
            }
        }
    });
}

fn render_effective_security(ui: &mut egui::Ui, snapshot: &PluginSettingsSnapshot) {
    let security = &snapshot.effective_security;
    if security.emergency_stop {
        ui.colored_label(
            egui::Color32::LIGHT_RED,
            i18n_embed_fl::fl!(crate::i18n::loader(), "plugins-emergency-stop"),
        );
    }
    for (category, mode) in &security.approvals {
        ui.label(format!("{category}: {mode}"));
    }
    if !security.fs_grants.is_empty() {
        for grant in &security.fs_grants {
            ui.label(format!(
                "{} → {} ({}{})",
                grant.slot,
                grant.path,
                if grant.read { "r" } else { "" },
                if grant.write { "w" } else { "" }
            ));
        }
    }
    ui.label(format!(
        "{}: {}",
        i18n_embed_fl::fl!(crate::i18n::loader(), "plugins-sandbox"),
        security.sandbox_enabled
    ));
    ui.label(format!(
        "{}: {}",
        i18n_embed_fl::fl!(crate::i18n::loader(), "plugins-db-quota"),
        security.db_quota_mb.map_or_else(
            || i18n_embed_fl::fl!(crate::i18n::loader(), "plugins-unbounded"),
            |mb| format!("{mb} MiB")
        )
    ));
}

/// Detected-but-unconfigured plugin binaries: one-click Add inserts a
/// default enabled entry into the draft's `plugins.list`.
fn render_discovered(
    ui: &mut egui::Ui,
    draft: &mut SettingsDraft,
    ai: &Arc<AiBridge>,
    input: &mut SettingsInputState,
) {
    input.discovered_plugins.poll();
    if !input.discovered_plugins.started() {
        input
            .discovered_plugins
            .start(ai.fetch_discovered_plugins());
    }
    let discovered = input.discovered_plugins.data.clone().unwrap_or_default();
    if discovered.is_empty() {
        return;
    }
    // Draft additions are already listed as placeholder rows; keep them out
    // of the discovered section until Apply moves them into the runtime
    // config (which is what the discovery fetch reads).
    let draft_plugins = draft.section::<ene_plugin_host::PluginConfig>();
    let pending: Vec<&String> = discovered
        .iter()
        .filter(|name| !draft_plugins.list.contains_key(*name))
        .collect();
    if pending.is_empty() {
        return;
    }
    ui.separator();
    components::section_card(
        ui,
        "plugins-discovered",
        &i18n_embed_fl::fl!(crate::i18n::loader(), "plugins-discovered-title"),
        |ui| {
            let mut add: Option<String> = None;
            for name in pending {
                ui.horizontal(|ui| {
                    ui.label(name.as_str());
                    ui.weak(i18n_embed_fl::fl!(
                        crate::i18n::loader(),
                        "plugins-discovered-hint"
                    ));
                    if ui
                        .small_button(i18n_embed_fl::fl!(crate::i18n::loader(), "plugins-add"))
                        .clicked()
                    {
                        add = Some(name.clone());
                    }
                });
            }
            if let Some(name) = add {
                let mut plugins = draft.section::<ene_plugin_host::PluginConfig>();
                plugins.list.entry(name.clone()).or_default();
                draft.set_section(&plugins);
                input.plugin_snapshots = AsyncData::new();
                input.discovered_plugins = AsyncData::new();
            }
        },
    );
}

fn render_mcp_section(
    ui: &mut egui::Ui,
    draft: &mut SettingsDraft,
    ai: &Arc<AiBridge>,
    input: &mut SettingsInputState,
) {
    input.mcp_statuses.poll();
    if !input.mcp_statuses.started() {
        input.mcp_statuses.start(ai.fetch_mcp_statuses());
    }
    let status_map: std::collections::BTreeMap<String, bool> = input
        .mcp_statuses
        .data
        .clone()
        .unwrap_or_default()
        .into_iter()
        .map(|status| (status.name, status.alive))
        .collect();
    components::section_card(
        ui,
        "plugins-mcp",
        &i18n_embed_fl::fl!(crate::i18n::loader(), "plugins-mcp-title"),
        |ui| {
            ui.weak(i18n_embed_fl::fl!(
                crate::i18n::loader(),
                "plugins-mcp-hint"
            ));
            let mut plugins = draft.section::<ene_plugin_host::PluginConfig>();
            let mut changed = false;
            let mut remove: Option<usize> = None;
            for (index, server) in plugins.mcp_servers.iter_mut().enumerate() {
                let alive = status_map.get(&server.name).copied();
                egui::CollapsingHeader::new(format!("mcp:{}", server.name))
                    .id_salt(("mcp_server", index))
                    .show(ui, |ui| {
                        if let Some(alive) = alive {
                            ui.horizontal(|ui| {
                                ui.label(i18n_embed_fl::fl!(
                                    crate::i18n::loader(),
                                    "plugins-mcp-status"
                                ));
                                if alive {
                                    ui.colored_label(
                                        egui::Color32::from_rgb(0x4c, 0xaf, 0x50),
                                        i18n_embed_fl::fl!(
                                            crate::i18n::loader(),
                                            "plugins-mcp-alive"
                                        ),
                                    );
                                } else {
                                    ui.colored_label(
                                        egui::Color32::from_rgb(0xf4, 0x43, 0x36),
                                        i18n_embed_fl::fl!(
                                            crate::i18n::loader(),
                                            "plugins-mcp-dead"
                                        ),
                                    );
                                }
                            });
                        }
                        let mut name = server.name.clone();
                        let mut enabled = server.enabled;
                        ui.horizontal(|ui| {
                            ui.label(i18n_embed_fl::fl!(crate::i18n::loader(), "plugins-name"));
                            if ui
                                .add(egui::TextEdit::singleline(&mut name).desired_width(180.0))
                                .changed()
                            {
                                server.name = name;
                                changed = true;
                            }
                            if ui
                                .checkbox(
                                    &mut enabled,
                                    i18n_embed_fl::fl!(crate::i18n::loader(), "plugins-enabled"),
                                )
                                .changed()
                            {
                                server.enabled = enabled;
                                changed = true;
                            }
                        });
                        match &mut server.transport {
                            ene_plugin_host::McpTransport::Stdio { command, args } => {
                                ui.horizontal(|ui| {
                                    ui.label(i18n_embed_fl::fl!(
                                        crate::i18n::loader(),
                                        "plugins-command"
                                    ));
                                    if ui
                                        .add(
                                            egui::TextEdit::singleline(command)
                                                .desired_width(220.0),
                                        )
                                        .changed()
                                    {
                                        changed = true;
                                    }
                                });
                                ui.horizontal(|ui| {
                                    ui.label(i18n_embed_fl::fl!(
                                        crate::i18n::loader(),
                                        "plugins-args"
                                    ));
                                    let mut args_text = args.join(" ");
                                    if ui
                                        .add(
                                            egui::TextEdit::singleline(&mut args_text)
                                                .desired_width(220.0),
                                        )
                                        .changed()
                                    {
                                        *args = args_text
                                            .split_whitespace()
                                            .map(str::to_string)
                                            .collect();
                                        changed = true;
                                    }
                                });
                            }
                            ene_plugin_host::McpTransport::Http { url, auth_header } => {
                                ui.horizontal(|ui| {
                                    ui.label(i18n_embed_fl::fl!(
                                        crate::i18n::loader(),
                                        "plugins-url"
                                    ));
                                    if ui
                                        .add(egui::TextEdit::singleline(url).desired_width(240.0))
                                        .changed()
                                    {
                                        changed = true;
                                    }
                                });
                                ui.horizontal(|ui| {
                                    ui.label(i18n_embed_fl::fl!(
                                        crate::i18n::loader(),
                                        "plugins-auth"
                                    ));
                                    let mut auth = auth_header.clone().unwrap_or_default();
                                    if ui
                                        .add(
                                            egui::TextEdit::singleline(&mut auth)
                                                .password(true)
                                                .desired_width(220.0),
                                        )
                                        .changed()
                                    {
                                        *auth_header = (!auth.is_empty()).then_some(auth);
                                        changed = true;
                                    }
                                });
                            }
                        }
                        if ui
                            .small_button(i18n_embed_fl::fl!(
                                crate::i18n::loader(),
                                "plugins-remove"
                            ))
                            .clicked()
                        {
                            remove = Some(index);
                        }
                    });
            }
            if let Some(index) = remove {
                plugins.mcp_servers.remove(index);
                changed = true;
            }
            if ui
                .small_button(i18n_embed_fl::fl!(crate::i18n::loader(), "plugins-add-mcp"))
                .on_hover_text(i18n_embed_fl::fl!(
                    crate::i18n::loader(),
                    "plugins-mcp-add-hint"
                ))
                .clicked()
            {
                plugins.mcp_servers.push(ene_plugin_host::McpServerConfig {
                    name: "new-server".to_string(),
                    enabled: true,
                    transport: ene_plugin_host::McpTransport::Stdio {
                        command: "npx".to_string(),
                        args: Vec::new(),
                    },
                    env_passthrough: Vec::new(),
                    sandbox: None,
                });
                changed = true;
            }
            if changed {
                draft.set_section(&plugins);
            }
        },
    );
}

fn toggle_row(ui: &mut egui::Ui, label: &str, hint: &str, value: &mut bool) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        if ui
            .add(egui::Checkbox::new(value, label))
            .on_hover_text(hint)
            .changed()
        {
            changed = true;
        }
    });
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(id: &str, kind: &str) -> PluginSettingsSnapshot {
        PluginSettingsSnapshot {
            id: id.to_string(),
            kind: kind.to_string(),
            enabled: true,
            health: PluginHealthState::Stopped,
            capabilities: ene_plugin_proto::PluginCapabilities::default(),
            actions: Vec::new(),
            manifest: ene_plugin_host::PluginManifestSummary {
                key_id: None,
                signed: false,
                checksum: None,
            },
            schema: None,
            schema_version: 0,
            supports_dynamic_config: false,
            supports_validate_config: false,
            config: None,
            profiles: None,
            credentials: Vec::new(),
            effective_security: ene_plugin_host::EffectiveSecurity::default(),
        }
    }

    #[test]
    fn stopped_builtins_classify_by_id() {
        assert_eq!(effective_kind(&snapshot("fs", "unknown")), "tool");
        assert_eq!(effective_kind(&snapshot("whisper", "unknown")), "provider");
        assert_eq!(effective_kind(&snapshot("custom", "unknown")), "unknown");
        assert_eq!(effective_kind(&snapshot("fs", "tool")), "tool");
        assert_eq!(
            effective_kind(&snapshot("anthropic", "provider")),
            "provider"
        );
    }

    #[test]
    fn every_kind_lands_on_a_tab() {
        assert!(is_tool_kind(effective_kind(&snapshot("fs", "unknown"))));
        assert!(!is_provider_kind(effective_kind(&snapshot(
            "fs", "unknown"
        ))));
        assert!(is_provider_kind(effective_kind(&snapshot(
            "whisper", "unknown"
        ))));
        assert!(!is_tool_kind(effective_kind(&snapshot(
            "whisper", "unknown"
        ))));
        assert!(is_tool_kind(effective_kind(&snapshot("custom", "unknown"))));
        assert!(is_tool_kind(effective_kind(&snapshot("browser", "hybrid"))));
        assert!(is_provider_kind(effective_kind(&snapshot(
            "browser", "hybrid"
        ))));
    }
}
