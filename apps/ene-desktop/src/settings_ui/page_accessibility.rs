//! Accessibility settings page — spotlight and caption overlay toggles.

use crate::settings::{CharacterSettings, DesktopSection};

pub fn render(ui: &mut egui::Ui, settings: &mut CharacterSettings) {
    ui.vertical(|ui| {
        ui.weak(i18n_embed_fl::fl!(
            crate::i18n::loader(),
            "accessibility-hint"
        ));
        ui.separator();

        let mut desktop = settings.config_section::<DesktopSection>();

        let mut spotlight_enabled = desktop.spotlight_enabled;
        if ui
            .checkbox(
                &mut spotlight_enabled,
                i18n_embed_fl::fl!(crate::i18n::loader(), "accessibility-spotlight"),
            )
            .on_hover_text(i18n_embed_fl::fl!(
                crate::i18n::loader(),
                "accessibility-spotlight-hint"
            ))
            .changed()
        {
            desktop.spotlight_enabled = spotlight_enabled;
            settings.set_config_section(&desktop);
            settings.mark_dirty();
        }

        let mut caption_enabled = desktop.caption_enabled;
        if ui
            .checkbox(
                &mut caption_enabled,
                i18n_embed_fl::fl!(crate::i18n::loader(), "accessibility-caption"),
            )
            .on_hover_text(i18n_embed_fl::fl!(
                crate::i18n::loader(),
                "accessibility-caption-hint"
            ))
            .changed()
        {
            desktop.caption_enabled = caption_enabled;
            settings.set_config_section(&desktop);
            settings.mark_dirty();
        }
    });
}
