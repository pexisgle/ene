use super::components::{section_card, toggle_row};
use crate::settings::{CharacterSettings, DesktopSection};

pub fn render(ui: &mut egui::Ui, settings: &mut CharacterSettings) {
    let mut desktop = settings.config_section::<DesktopSection>();
    let mut changed = false;

    section_card(
        ui,
        "accessibility-overlays",
        &i18n_embed_fl::fl!(crate::i18n::loader(), "section-accessibility-overlays"),
        |ui| {
            let mut spotlight_enabled = desktop.spotlight_enabled;
            if toggle_row(
                ui,
                "accessibility_spotlight",
                &i18n_embed_fl::fl!(crate::i18n::loader(), "accessibility-spotlight"),
                &i18n_embed_fl::fl!(crate::i18n::loader(), "accessibility-spotlight-hint"),
                &mut spotlight_enabled,
            ) {
                desktop.spotlight_enabled = spotlight_enabled;
                changed = true;
            }

            let mut caption_enabled = desktop.caption_enabled;
            if toggle_row(
                ui,
                "accessibility_caption",
                &i18n_embed_fl::fl!(crate::i18n::loader(), "accessibility-caption"),
                &i18n_embed_fl::fl!(crate::i18n::loader(), "accessibility-caption-hint"),
                &mut caption_enabled,
            ) {
                desktop.caption_enabled = caption_enabled;
                changed = true;
            }
        },
    );

    if changed {
        settings.set_config_section(&desktop);
        settings.mark_dirty();
    }
}
