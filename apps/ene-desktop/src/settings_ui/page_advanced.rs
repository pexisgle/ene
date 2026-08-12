//! Advanced page: a searchable generic editor for every registered settings
//! schema leaf that lacks a dedicated page.
//!
//! This is the completeness guarantee behind "every schema item is
//! reachable from the GUI": whatever a dedicated page does not cover, the
//! schema form renders here, with unknown-key preservation and a raw JSON
//! fallback.

use super::components;
use super::draft::SettingsDraft;
use super::schema_form::{SchemaFormOptions, schema_object_form};

/// Renders the advanced page. `show_advanced` reveals `x-ene-ui.advanced`
/// fields (the window-level search activates it).
pub fn render(ui: &mut egui::Ui, draft: &mut SettingsDraft, show_advanced: bool) {
    ui.weak(i18n_embed_fl::fl!(crate::i18n::loader(), "advanced-hint"));
    ui.add_space(6.0);

    let mut filter = ui.data_mut(|data| {
        data.get_temp::<String>(egui::Id::new("advanced_filter"))
            .unwrap_or_default()
    });
    ui.horizontal(|ui| {
        ui.label(i18n_embed_fl::fl!(crate::i18n::loader(), "advanced-filter"));
        let response = ui.add(egui::TextEdit::singleline(&mut filter).desired_width(240.0));
        if response.changed() {
            ui.data_mut(|data| {
                data.insert_temp(egui::Id::new("advanced_filter"), filter.clone());
            });
        }
    });
    ui.add_space(4.0);

    let sections = ene_config::config::registered_schemas_for(ene_config::ConfigTarget::Settings);
    let mut keys: Vec<String> = sections.iter().map(|(key, _)| key.clone()).collect();
    keys.sort();

    components::section_card_collapsible(
        ui,
        "advanced-sections",
        &i18n_embed_fl::fl!(crate::i18n::loader(), "advanced-sections"),
        true,
        |ui| {
            for key in keys {
                if !filter.trim().is_empty() && !key.contains(filter.trim()) {
                    continue;
                }
                let Some((_, entry)) = sections.iter().find(|(candidate, _)| *candidate == key)
                else {
                    continue;
                };
                let Ok(schema) = serde_json::to_value(&entry.schema) else {
                    continue;
                };
                let unsupported = super::schema_form::unsupported_schema_constructs(&schema);
                let mut value = draft
                    .editing()
                    .section_value(&key)
                    .unwrap_or_else(|| serde_json::Value::Object(serde_json::Map::new()));
                let mut form_changed = false;
                let impact = SettingsDraft::impact_for(&key);
                components::section_card_collapsible(
                    ui,
                    &format!("advanced-{key}"),
                    &format!("{key} ({})", impact.code()),
                    false,
                    |ui| {
                        if !unsupported.is_empty() {
                            ui.colored_label(
                                egui::Color32::from_rgb(0xff, 0x8a, 0x65),
                                format!(
                                    "{}: {}",
                                    i18n_embed_fl::fl!(
                                        crate::i18n::loader(),
                                        "advanced-unsupported"
                                    ),
                                    unsupported.join(", ")
                                ),
                            );
                        }
                        let issues = draft.issues_for(&key);
                        if !issues.is_empty() {
                            for issue in issues {
                                ui.colored_label(egui::Color32::from_rgb(0xff, 0x8a, 0x65), issue);
                            }
                        }
                        form_changed |= schema_object_form(
                            ui,
                            &schema,
                            &mut value,
                            &key,
                            SchemaFormOptions {
                                show_advanced,
                                show_impact: true,
                                epoch: draft.applied_revision(),
                                options: None,
                            },
                        );
                    },
                );
                if form_changed {
                    draft.set_section_value(&key, value);
                }
            }
        },
    );
}
