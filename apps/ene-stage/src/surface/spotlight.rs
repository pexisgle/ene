//! Alt+Space command palette.

use crate::detail::DetailTab;
use crate::i18n;
use crate::shell::ShellCommand;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpotlightAction {
    Command(ShellCommand),
    Close,
}

impl SpotlightAction {
    /// Quick commands leave the destination in front; the palette must not stay open.
    #[must_use]
    pub const fn dismisses_palette(self) -> bool {
        match self {
            Self::Command(_) | Self::Close => true,
        }
    }
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
                    action = Some(SpotlightAction::Command(ShellCommand::OpenDetail(tab)));
                }
            }
            if ui.button(i18n::fl("spotlight-toggle-mic")).clicked() {
                action = Some(SpotlightAction::Command(ShellCommand::ToggleMic));
            }
            if ui.button(i18n::fl("spotlight-quit")).clicked() {
                action = Some(SpotlightAction::Command(ShellCommand::Quit));
            }
            if ui.button(i18n::fl("spotlight-close")).clicked() {
                action = Some(SpotlightAction::Close);
            }
        });
    action
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_quick_command_dismisses_the_palette() {
        for tab in DetailTab::ALL {
            assert!(SpotlightAction::Command(ShellCommand::OpenDetail(tab)).dismisses_palette());
        }
        assert!(SpotlightAction::Command(ShellCommand::ToggleMic).dismisses_palette());
        assert!(SpotlightAction::Command(ShellCommand::Quit).dismisses_palette());
        assert!(SpotlightAction::Close.dismisses_palette());
    }
}
