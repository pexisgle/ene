//! Graphics settings page.
//!
//! Quality preset, UI language, and the app-wide color theme.
use super::components::section_card;
use crate::settings::{
    CharacterSettings, DesktopThemePreference, GraphicsQuality, GraphicsSettings, Language,
};

fn language_label(language: Language) -> &'static str {
    match language {
        Language::En => "English",
        Language::Ja => "日本語",
    }
}

pub fn render(
    ui: &mut egui::Ui,
    settings: &mut CharacterSettings,
    _animation: &mut crate::character_state::AnimationControl,
    _ai: &std::sync::Arc<crate::ai_bridge::AiBridge>,
    _world: &mut bevy_ecs::world::World,
    _ui_entity: bevy_ecs::entity::Entity,
) {
    section_card(
        ui,
        "graphics-quality",
        &i18n_embed_fl::fl!(crate::i18n::loader(), "section-graphics-quality"),
        |ui| {
            let mut quality = settings.graphics().quality;
            egui::ComboBox::from_id_salt("graphics_quality_combo")
                .selected_text(super::widgets::format_quality_label(
                    settings.language(),
                    quality,
                ))
                .width(220.0)
                .show_ui(ui, |ui| {
                    for candidate in [
                        GraphicsQuality::Low,
                        GraphicsQuality::Medium,
                        GraphicsQuality::High,
                    ] {
                        ui.selectable_value(
                            &mut quality,
                            candidate,
                            super::widgets::format_quality_label(settings.language(), candidate),
                        );
                    }
                });
            if quality != settings.graphics().quality {
                settings.set_graphics(GraphicsSettings { quality });
                settings.mark_dirty();
            }
        },
    );

    section_card(
        ui,
        "graphics-language",
        &i18n_embed_fl::fl!(crate::i18n::loader(), "section-graphics-language"),
        |ui| {
            let mut language = settings.language();
            egui::ComboBox::from_id_salt("graphics_language_combo")
                .selected_text(language_label(language))
                .width(220.0)
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut language, Language::En, "English");
                    ui.selectable_value(&mut language, Language::Ja, "日本語");
                });
            if language != settings.language() {
                settings.set_language(language);
                crate::i18n::select_language(language);
                settings.sync_classifier_language_from_ui();
                settings.mark_dirty();
            }
        },
    );

    section_card(
        ui,
        "graphics-theme",
        &i18n_embed_fl::fl!(crate::i18n::loader(), "section-graphics-theme"),
        |ui| {
            ui.weak(i18n_embed_fl::fl!(crate::i18n::loader(), "theme-hint"));
            ui.add_space(4.0);
            let mut theme = settings.theme();
            egui::ComboBox::from_id_salt("desktop_theme_combo")
                .selected_text(theme_label(theme))
                .width(220.0)
                .show_ui(ui, |ui| {
                    ui.selectable_value(
                        &mut theme,
                        DesktopThemePreference::System,
                        i18n_embed_fl::fl!(crate::i18n::loader(), "theme-system"),
                    );
                    ui.selectable_value(
                        &mut theme,
                        DesktopThemePreference::Light,
                        i18n_embed_fl::fl!(crate::i18n::loader(), "theme-light"),
                    );
                    ui.selectable_value(
                        &mut theme,
                        DesktopThemePreference::Dark,
                        i18n_embed_fl::fl!(crate::i18n::loader(), "theme-dark"),
                    );
                });
            if theme != settings.theme() {
                settings.set_theme(theme);
                settings.mark_dirty();
            }
        },
    );
}

fn theme_label(theme: DesktopThemePreference) -> String {
    match theme {
        DesktopThemePreference::System => {
            i18n_embed_fl::fl!(crate::i18n::loader(), "theme-system")
        }
        DesktopThemePreference::Light => {
            i18n_embed_fl::fl!(crate::i18n::loader(), "theme-light")
        }
        DesktopThemePreference::Dark => {
            i18n_embed_fl::fl!(crate::i18n::loader(), "theme-dark")
        }
    }
}
