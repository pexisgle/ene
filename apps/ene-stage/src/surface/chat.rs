//! Chat panel for the surface viewport.

use crate::detail::DetailTab;
use crate::i18n;
use crate::surface::{SurfaceAction, SurfaceUiState};
use ene_api::MessageMode;

pub fn show(ui: &mut egui::Ui, state: &mut SurfaceUiState, mic_active: bool) -> egui::Response {
    let output = ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
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
            ui.add_enabled_ui(state.turn_active, |ui| {
                if ui
                    .button(i18n::fl("chat-barge-in"))
                    .on_hover_text(i18n::fl("chat-barge-in-hint"))
                    .clicked()
                {
                    state.push_action(SurfaceAction::BargeIn);
                }
                if ui
                    .button(i18n::fl("chat-cancel"))
                    .on_hover_text(i18n::fl("chat-cancel-hint"))
                    .clicked()
                {
                    state.push_action(SurfaceAction::CancelTurn);
                }
            });
            if ui
                .button(i18n::fl("chat-open-detail"))
                .on_hover_text(i18n::fl("chat-open-detail-hint"))
                .clicked()
            {
                state.push_action(SurfaceAction::OpenDetail(DetailTab::Home));
            }
        });
        if !state.status.is_empty() {
            ui.add(
                egui::Label::new(egui::RichText::new(&state.status).small())
                    .wrap()
                    .selectable(true),
            );
        }
        ui.horizontal(|ui| {
            ui.label(i18n::fl("chat-mode"));
            for (mode, label, hint) in [
                (
                    MessageMode::Prompt,
                    i18n::fl("chat-mode-prompt"),
                    i18n::fl("chat-mode-prompt-hint"),
                ),
                (
                    MessageMode::Steer,
                    i18n::fl("chat-mode-steer"),
                    i18n::fl("chat-mode-steer-hint"),
                ),
                (
                    MessageMode::FollowUp,
                    i18n::fl("chat-mode-follow-up"),
                    i18n::fl("chat-mode-follow-up-hint"),
                ),
            ] {
                let enabled = mode == MessageMode::Prompt || state.turn_active;
                ui.add_enabled_ui(enabled, |ui| {
                    if ui
                        .selectable_label(state.message_mode == mode, label)
                        .on_hover_text(hint)
                        .clicked()
                    {
                        state.message_mode = mode;
                    }
                });
            }
            if !state.voice_state.is_empty() {
                ui.label(format!(
                    "{}: {}",
                    i18n::fl("voice-state"),
                    state.voice_state
                ));
            }
        });
        ui.label(i18n::fl("chat-overlay-hint"));
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
                if let Some(error) = state.terminal_error() {
                    egui::Frame::default()
                        .fill(egui::Color32::from_rgb(60, 24, 24))
                        .inner_margin(8.0)
                        .corner_radius(6.0)
                        .show(ui, |ui| {
                            ui.set_min_width(ui.available_width());
                            ui.colored_label(
                                egui::Color32::from_rgb(255, 205, 205),
                                format!("{}: {error}", i18n::fl("status-error")),
                            );
                        });
                    ui.add_space(4.0);
                }
                for message in &state.history.messages {
                    if message.role != "user" && message.role != "assistant" {
                        continue;
                    }
                    ui.label(format!("{}: {}", message.role, message.text));
                }
                if !state.streaming_text.is_empty() {
                    ui.colored_label(
                        egui::Color32::LIGHT_GREEN,
                        format!("assistant: {}", state.streaming_text),
                    );
                }
            });

        response
    });

    output.inner
}
