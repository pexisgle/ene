//! Plugin approval policy settings page.
//!
//! Shows the global per-category policy table, per-plugin overrides, the
//! high-risk warning, the emergency stop, and the one-click reset to `Ask`.
//! Every change is persisted through the normal settings section path; the
//! broker hub rebuilds its resolver from the same config on the next start.

use std::sync::Arc;

use bevy_ecs::world::World;
use ene_plugin_host::{ALL_CATEGORIES, ApprovalCategory, ApprovalMode, PluginConfig};
use i18n_embed_fl::fl;

use crate::ai_bridge::AiBridge;
use crate::settings::CharacterSettings;
use crate::settings_ui::components::{section_card, warning_box};

pub fn render(
    ui: &mut egui::Ui,
    settings: &mut CharacterSettings,
    _ai: &Arc<AiBridge>,
    _world: &mut World,
    _ui_entity: bevy_ecs::entity::Entity,
) {
    section_card(
        ui,
        "approvals-policy",
        &fl!(crate::i18n::loader(), "approvals-title"),
        |ui| {
            ui.label(fl!(crate::i18n::loader(), "approvals-hint"));

            let mut config = settings.config_section::<PluginConfig>();
            let mut changed = false;

            if config.approval.has_high_risk_allow() {
                warning_box(
                    ui,
                    &fl!(crate::i18n::loader(), "approvals-high-risk-warning"),
                );
                ui.add_space(4.0);
            }

            let mut emergency = config.approval.emergency_stop;
            if ui
                .checkbox(
                    &mut emergency,
                    fl!(crate::i18n::loader(), "approvals-emergency"),
                )
                .changed()
            {
                config.approval.emergency_stop = emergency;
                changed = true;
            }
            ui.label(fl!(crate::i18n::loader(), "approvals-emergency-hint"));
            ui.separator();

            ui.label(fl!(crate::i18n::loader(), "approvals-global"));
            render_category_table(ui, &mut config, None, &mut changed);

            if ui
                .button(fl!(crate::i18n::loader(), "approvals-reset"))
                .clicked()
            {
                config.approval = ene_approval::ApprovalPolicy::default();
                changed = true;
            }
            ui.separator();

            ui.label(fl!(crate::i18n::loader(), "approvals-per-plugin"));
            let mut names: Vec<String> = config.list.keys().cloned().collect();
            names.sort();
            if names.is_empty() {
                ui.weak(fl!(crate::i18n::loader(), "approvals-no-plugins"));
            }
            for name in names {
                egui::CollapsingHeader::new(&name)
                    .id_salt(("approval_plugin", name.as_str()))
                    .show(ui, |ui| {
                        render_category_table(ui, &mut config, Some(&name), &mut changed);
                    });
            }

            if let Some(path) = &config.audit_log_path {
                ui.separator();
                ui.label(format!(
                    "{} {path}",
                    fl!(crate::i18n::loader(), "approvals-audit-path")
                ));
            }

            if changed {
                settings.set_config_section(&config);
                settings.mark_dirty();
            }
        },
    );
}

/// Renders one category × mode row grid. With `plugin` set, edits the
/// per-plugin override; without, edits the global policy.
fn render_category_table(
    ui: &mut egui::Ui,
    config: &mut PluginConfig,
    plugin: Option<&str>,
    changed: &mut bool,
) {
    egui::Grid::new(egui::Id::new(("approval_grid", plugin)))
        .striped(true)
        .num_columns(2)
        .show(ui, |ui| {
            for category in ALL_CATEGORIES {
                ui.label(category_name(*category));
                let current = match plugin {
                    Some(name) => config
                        .plugin_approval
                        .get(name)
                        .and_then(|policy| policy.categories.get(category))
                        .copied()
                        .unwrap_or(ApprovalMode::Inherit),
                    None => config
                        .approval
                        .categories
                        .get(category)
                        .copied()
                        .unwrap_or(ApprovalMode::Ask),
                };
                let mut selected = current;
                egui::ComboBox::from_id_salt(("approval_mode", plugin, category))
                    .selected_text(mode_label(selected))
                    .show_ui(ui, |ui| {
                        for mode in mode_choices(plugin.is_some()) {
                            if ui
                                .selectable_label(selected == mode, mode_label(mode))
                                .clicked()
                            {
                                selected = mode;
                            }
                        }
                    });
                if selected != current {
                    match plugin {
                        Some(name) => {
                            config
                                .plugin_approval
                                .entry(name.to_string())
                                .or_default()
                                .categories
                                .insert(*category, selected);
                        }
                        None => {
                            config.approval.categories.insert(*category, selected);
                        }
                    }
                    *changed = true;
                }
                ui.end_row();
            }
        });
}

fn mode_choices(include_inherit: bool) -> Vec<ApprovalMode> {
    let mut choices = vec![ApprovalMode::Ask, ApprovalMode::Allow, ApprovalMode::Deny];
    if include_inherit {
        choices.insert(0, ApprovalMode::Inherit);
    }
    choices
}

fn mode_label(mode: ApprovalMode) -> String {
    match mode {
        ApprovalMode::Inherit => fl!(crate::i18n::loader(), "approvals-inherit").to_string(),
        ApprovalMode::Ask => fl!(crate::i18n::loader(), "approvals-ask").to_string(),
        ApprovalMode::Allow => fl!(crate::i18n::loader(), "approvals-allow").to_string(),
        ApprovalMode::Deny => fl!(crate::i18n::loader(), "approvals-deny").to_string(),
    }
}

fn category_name(category: ApprovalCategory) -> String {
    // Raw snake_case names keep the table compact and greppable against
    // `settings.json`; the structural labels above are localized.
    format!("{category:?}")
        .chars()
        .flat_map(|c| {
            if c.is_uppercase() {
                vec!['_', c.to_ascii_lowercase()]
            } else {
                vec![c]
            }
        })
        .collect()
}
