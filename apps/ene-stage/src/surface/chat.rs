//! Chat panel for the surface viewport.

use eframe::egui::{self, ScrollArea, TextEdit};

use crate::i18n;
use crate::surface::{SurfaceAction, SurfaceUiState};

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

        ui.add(
            TextEdit::singleline(&mut state.chat_draft)
                .id_salt("stage-chat-input")
                .hint_text(i18n::fl("chat-placeholder"))
                .desired_width(ui.available_width()),
        );

        ui.add_space(4.0);
        ScrollArea::vertical()
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
