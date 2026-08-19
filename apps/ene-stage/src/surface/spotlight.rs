//! Alt+Space command palette.

use eframe::egui;

use crate::detail::DetailTab;
use crate::i18n;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpotlightAction {
    OpenDetail(DetailTab),
    ToggleMic,
    Quit,
    Close,
}

pub fn show(ctx: &egui::Context) -> Option<SpotlightAction> {
    let mut action = None;
    egui::Window::new(i18n::fl("spotlight-title"))
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.label(i18n::fl("spotlight-hint"));
            ui.separator();
            if ui.button(i18n::fl("detail-tab-log")).clicked() {
                action = Some(SpotlightAction::OpenDetail(DetailTab::Log));
            }
            if ui.button(i18n::fl("detail-tab-settings")).clicked() {
                action = Some(SpotlightAction::OpenDetail(DetailTab::Settings));
            }
            if ui.button(i18n::fl("detail-tab-memory")).clicked() {
                action = Some(SpotlightAction::OpenDetail(DetailTab::Memory));
            }
            if ui.button(i18n::fl("detail-tab-character")).clicked() {
                action = Some(SpotlightAction::OpenDetail(DetailTab::Character));
            }
            if ui.button(i18n::fl("detail-tab-jobs")).clicked() {
                action = Some(SpotlightAction::OpenDetail(DetailTab::Jobs));
            }
            if ui.button(i18n::fl("detail-tab-plugins")).clicked() {
                action = Some(SpotlightAction::OpenDetail(DetailTab::Plugins));
            }
            if ui.button(i18n::fl("spotlight-toggle-mic")).clicked() {
                action = Some(SpotlightAction::ToggleMic);
            }
            if ui.button(i18n::fl("spotlight-quit")).clicked() {
                action = Some(SpotlightAction::Quit);
            }
            if ui.button(i18n::fl("spotlight-close")).clicked() {
                action = Some(SpotlightAction::Close);
            }
        });
    action
}
