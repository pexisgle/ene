use crate::ai_bridge::AiBridge;
use crate::settings::CharacterSettings;

use super::components;
use super::input::SettingsInputState;
use super::{ApplyFeedback, PageKind};
use ene_plugin_host::PluginHealthState;
use std::sync::Arc;

/// `current_page` lets the page navigate directly.
pub fn render(
    ui: &mut egui::Ui,
    settings: &CharacterSettings,
    ai: &Arc<AiBridge>,
    input: &mut SettingsInputState,
    current_page: &mut PageKind,
    feedback: Option<&ApplyFeedback>,
) {
    input.plugin_snapshots.poll();
    if !input.plugin_snapshots.started() {
        input.plugin_snapshots.start(ai.fetch_plugin_snapshots());
    }
    let snapshots = input.plugin_snapshots.data.clone().unwrap_or_default();

    let mut navigate: Option<PageKind> = None;

    let config = settings.config();
    components::section_card(
        ui,
        "overview-needs-config",
        &i18n_embed_fl::fl!(crate::i18n::loader(), "overview-needs-config"),
        |ui| {
            if ene_ai::needs_onboarding(&config) {
                issue_row(
                    ui,
                    &i18n_embed_fl::fl!(crate::i18n::loader(), "overview-onboarding"),
                    &i18n_embed_fl::fl!(crate::i18n::loader(), "overview-onboarding-hint"),
                    &mut navigate,
                    PageKind::Ai,
                );
            } else {
                ui.weak(i18n_embed_fl::fl!(
                    crate::i18n::loader(),
                    "overview-all-set"
                ));
            }
        },
    );

    components::section_card(
        ui,
        "overview-restart-pending",
        &i18n_embed_fl::fl!(crate::i18n::loader(), "overview-restart-pending"),
        |ui| {
            let pending =
                feedback.is_some_and(|f| f.ok && (f.impact.plugin_restart || f.impact.app_restart));
            if let Some(feedback) = feedback
                && feedback.ok
                && feedback.impact.plugin_restart
            {
                issue_row(
                    ui,
                    &i18n_embed_fl::fl!(crate::i18n::loader(), "overview-plugin-restart"),
                    "",
                    &mut navigate,
                    PageKind::Plugins,
                );
            }
            if let Some(feedback) = feedback
                && feedback.ok
                && feedback.impact.app_restart
            {
                issue_row(
                    ui,
                    &i18n_embed_fl::fl!(crate::i18n::loader(), "overview-app-restart"),
                    "",
                    &mut navigate,
                    PageKind::Advanced,
                );
            }
            if !pending {
                ui.weak(i18n_embed_fl::fl!(
                    crate::i18n::loader(),
                    "overview-no-restart"
                ));
            }
        },
    );

    components::section_card(
        ui,
        "overview-issues",
        &i18n_embed_fl::fl!(crate::i18n::loader(), "overview-issues"),
        |ui| {
            let mut issue_count = 0;
            for snapshot in &snapshots {
                match snapshot.health {
                    PluginHealthState::Running | PluginHealthState::Stopped => {}
                    PluginHealthState::Disabled => {
                        issue_count += 1;
                        issue_row(
                            ui,
                            &format!("{}: disabled", snapshot.id),
                            "",
                            &mut navigate,
                            PageKind::Plugins,
                        );
                    }
                    PluginHealthState::RequirementsUnmet => {
                        issue_count += 1;
                        issue_row(
                            ui,
                            &format!("{}: requirements unmet", snapshot.id),
                            "",
                            &mut navigate,
                            PageKind::Plugins,
                        );
                    }
                }
            }
            if issue_count == 0 {
                ui.weak(i18n_embed_fl::fl!(
                    crate::i18n::loader(),
                    "overview-healthy"
                ));
            }
        },
    );

    components::section_card(
        ui,
        "overview-credentials",
        &i18n_embed_fl::fl!(crate::i18n::loader(), "overview-credentials"),
        |ui| {
            let mut credential_rows = 0;
            for snapshot in &snapshots {
                for declaration in &snapshot.credentials {
                    if declaration.required {
                        credential_rows += 1;
                        issue_row(
                            ui,
                            &format!("{}: {}", snapshot.id, declaration.id),
                            declaration.help_url.as_deref().unwrap_or_default(),
                            &mut navigate,
                            PageKind::Plugins,
                        );
                    }
                }
            }
            if credential_rows == 0 {
                ui.weak(i18n_embed_fl::fl!(
                    crate::i18n::loader(),
                    "overview-no-credentials"
                ));
            }
        },
    );

    if let Some(page) = navigate {
        *current_page = page;
    }
}

fn issue_row(
    ui: &mut egui::Ui,
    label: &str,
    detail: &str,
    navigate: &mut Option<PageKind>,
    target: PageKind,
) {
    ui.horizontal(|ui| {
        ui.colored_label(egui::Color32::from_rgb(0xff, 0x98, 0x00), "●");
        ui.label(label);
        if !detail.is_empty() {
            ui.weak(detail);
        }
        if ui
            .small_button(i18n_embed_fl::fl!(crate::i18n::loader(), "overview-open"))
            .clicked()
        {
            *navigate = Some(target);
        }
    });
}
