//! Alt+Space command palette.

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
            for tab in DetailTab::ALL {
                if ui.button(tab.label()).clicked() {
                    action = Some(SpotlightAction::OpenDetail(tab));
                }
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
