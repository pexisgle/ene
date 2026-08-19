//! Chat panel for the surface viewport.

use crate::i18n;
use crate::surface::{SurfaceAction, SurfaceUiState};
use ene_api::MessageMode;

pub fn show(ui: &mut egui::Ui, state: &mut SurfaceUiState, mic_active: bool) {
    ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
        ui.horizontal(|ui| {
            if ui.button(i18n::fl("chat-send")).clicked() {
                state.push_action(SurfaceAction::SendChat);
            }
            let mic_label = if mic_active {
                i18n::fl("mic-on")
            } else {
                i18n::fl("mic-off")
            };
            if ui.button(mic_label).clicked() {
                state.push_action(SurfaceAction::ToggleMic);
            }
            if ui.button(i18n::fl("chat-barge-in")).clicked() {
                state.push_action(SurfaceAction::BargeIn);
            }
            if ui.button(i18n::fl("chat-cancel")).clicked() {
                state.push_action(SurfaceAction::CancelTurn);
            }
            ui.label(&state.status);
        });
        ui.horizontal(|ui| {
            ui.label(i18n::fl("chat-mode"));
            for (mode, label) in [
                (MessageMode::Prompt, i18n::fl("chat-mode-prompt")),
                (MessageMode::Steer, i18n::fl("chat-mode-steer")),
                (MessageMode::FollowUp, i18n::fl("chat-mode-follow-up")),
            ] {
                if ui
                    .selectable_label(state.message_mode == mode, label)
                    .clicked()
                {
                    state.message_mode = mode;
                }
            }
            if !state.voice_state.is_empty() {
                ui.label(format!(
                    "{}: {}",
                    i18n::fl("voice-state"),
                    state.voice_state
                ));
            }
        });
        if !state.exclusive_notice.is_empty() {
            ui.colored_label(egui::Color32::YELLOW, &state.exclusive_notice);
        }

        let response = ui.add(
            egui::TextEdit::singleline(&mut state.chat_draft)
                .id_salt("stage-chat-input")
                .hint_text(i18n::fl("chat-placeholder"))
                .desired_width(ui.available_width()),
        );
        if response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            state.push_action(SurfaceAction::SendChat);
        }

        ui.add_space(4.0);
        egui::ScrollArea::vertical()
            .max_height(ui.available_height() - 8.0)
            .stick_to_bottom(true)
            .show(ui, |ui| {
                for message in &state.history.messages {
                    ui.label(format!("{}: {}", message.role, message.text));
                }
                if !state.streaming_text.is_empty() {
                    ui.colored_label(
                        egui::Color32::LIGHT_GREEN,
                        format!("assistant: {}", state.streaming_text),
                    );
                }
            });
    });
}
