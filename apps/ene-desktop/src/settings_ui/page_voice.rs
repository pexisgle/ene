use std::sync::Arc;

use crate::core_session::CoreSession;
use crate::settings::CharacterSettings;

use super::components::{section_card, toggle_row};
use super::draft::SettingsDraft;
use super::input::SettingsInputState;

pub fn render(
    ui: &mut egui::Ui,
    settings: &mut CharacterSettings,
    _draft: &mut SettingsDraft,
    ai: &Arc<CoreSession>,
    input: &mut SettingsInputState,
    world: &mut bevy_ecs::world::World,
) {
    section_card(
        ui,
        "voice-mic",
        &i18n_embed_fl::fl!(crate::i18n::loader(), "audio-mic-section"),
        |ui| {
            #[cfg(feature = "voice")]
            {
                if input.mic_devices.is_empty() {
                    input.mic_devices = crate::audio::list_input_device_names();
                }
                let mut selected = settings.mic_device();
                let label = selected
                    .clone()
                    .unwrap_or_else(|| i18n_embed_fl::fl!(crate::i18n::loader(), "match-window"));
                egui::ComboBox::from_id_salt("mic_device")
                    .selected_text(label)
                    .show_ui(ui, |ui| {
                        if ui
                            .selectable_label(
                                selected.is_none(),
                                i18n_embed_fl::fl!(crate::i18n::loader(), "match-window"),
                            )
                            .clicked()
                        {
                            selected = None;
                        }
                        for name in &input.mic_devices {
                            if ui
                                .selectable_label(selected.as_deref() == Some(name), name)
                                .clicked()
                            {
                                selected = Some(name.clone());
                            }
                        }
                    });
                if selected != settings.mic_device() {
                    settings.set_mic_device(selected);
                    settings.mark_dirty();
                }

                let mut beat_sync = settings
                    .config_section::<crate::settings::DesktopSection>()
                    .beat_sync
                    .enabled;
                if toggle_row(
                    ui,
                    "beat_sync",
                    &i18n_embed_fl::fl!(crate::i18n::loader(), "beat-sync-enabled"),
                    &i18n_embed_fl::fl!(crate::i18n::loader(), "beat-sync-hint"),
                    &mut beat_sync,
                ) {
                    settings.set_beat_sync_enabled(beat_sync);
                    settings.mark_dirty();
                    if let Err(err) = crate::audio::set_beat_sync_enabled(
                        world,
                        ai,
                        beat_sync,
                        settings.beat_sync_device(),
                    ) {
                        tracing::warn!(error = %err, "beat sync toggle failed");
                    }
                }
            }
            #[cfg(not(feature = "voice"))]
            {
                drop((ai, world));
                ui.weak(i18n_embed_fl::fl!(
                    crate::i18n::loader(),
                    "voice-feature-disabled"
                ));
            }
        },
    );
}
