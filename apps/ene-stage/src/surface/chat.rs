//! Chat panel for the surface viewport.

use crate::detail::DetailTab;
use crate::i18n;
use crate::surface::{SurfaceAction, SurfaceUiState};
use ene_api::{HistoryResponse, MessageMode, MessageResponse};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TranscriptKind {
    User,
    Assistant,
    Error,
    Tool,
    System,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TranscriptState {
    Stable,
    Error,
    Streaming,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TranscriptRow<'a> {
    kind: TranscriptKind,
    state: TranscriptState,
    text: &'a str,
}

fn normalize_transcript<'a>(
    history: &'a HistoryResponse,
    streaming_text: &'a str,
) -> Vec<TranscriptRow<'a>> {
    let mut rows = history
        .messages
        .iter()
        .filter_map(normalize_message)
        .collect::<Vec<_>>();
    if !streaming_text.is_empty() {
        rows.push(TranscriptRow {
            kind: TranscriptKind::Assistant,
            state: TranscriptState::Streaming,
            text: streaming_text,
        });
    }
    rows
}

fn normalize_message(message: &MessageResponse) -> Option<TranscriptRow<'_>> {
    let (kind, state) = match message.role.as_str() {
        "user" => (TranscriptKind::User, TranscriptState::Stable),
        "assistant" => (TranscriptKind::Assistant, TranscriptState::Stable),
        "status" | "error" => (TranscriptKind::Error, TranscriptState::Error),
        "tool" | "tool-summary" => (TranscriptKind::Tool, TranscriptState::Stable),
        "inner" | "thinking" => return None,
        _ => (TranscriptKind::System, TranscriptState::Stable),
    };
    Some(TranscriptRow {
        kind,
        state,
        text: &message.text,
    })
}

fn transcript_label(kind: TranscriptKind) -> String {
    i18n::fl(match kind {
        TranscriptKind::User => "chat-role-user",
        TranscriptKind::Assistant => "chat-role-assistant",
        TranscriptKind::Error => "chat-error",
        TranscriptKind::Tool => "chat-tool",
        TranscriptKind::System => "chat-system",
    })
}

fn render_transcript_row(ui: &mut egui::Ui, row: TranscriptRow<'_>) {
    let is_user = row.kind == TranscriptKind::User;
    let frame_color = match row.state {
        TranscriptState::Error => egui::Color32::from_rgb(76, 29, 29),
        TranscriptState::Stable | TranscriptState::Streaming if is_user => {
            egui::Color32::from_rgb(52, 90, 130)
        }
        TranscriptState::Stable | TranscriptState::Streaming => egui::Color32::from_rgb(38, 42, 50),
    };
    let text_color = if row.state == TranscriptState::Error {
        egui::Color32::from_rgb(255, 205, 205)
    } else {
        egui::Color32::PLACEHOLDER
    };
    let row_width = ui.available_width();
    let bubble_max_width = (row_width * 0.82).max(120.0);
    let frame = egui::Frame::new()
        .fill(frame_color)
        .inner_margin(egui::Margin::symmetric(10, 8))
        .corner_radius(8.0);
    let align = if is_user {
        egui::Align::Max
    } else {
        egui::Align::Min
    };

    ui.with_layout(egui::Layout::top_down(align), |ui| {
        ui.set_width(row_width);
        frame.show(ui, |ui| {
            ui.set_max_width(bubble_max_width);
            ui.label(
                egui::RichText::new(transcript_label(row.kind))
                    .small()
                    .weak(),
            );
            let mut text = row.text.to_owned();
            if row.state == TranscriptState::Streaming {
                text.push('▌');
            }
            if text.is_empty() && row.state == TranscriptState::Streaming {
                ui.weak(i18n::fl("chat-waiting"));
            } else {
                ui.add(
                    egui::Label::new(egui::RichText::new(text).color(text_color))
                        .wrap()
                        .selectable(true),
                );
            }
        });
    });
    ui.add_space(6.0);
}

fn render_greeting_picker(ui: &mut egui::Ui, state: &mut SurfaceUiState) {
    if state.greetings.is_empty() {
        ui.weak(i18n::fl("chat-empty-history"));
        return;
    }
    ui.label(i18n::fl("chat-greeting-prompt"));
    for greeting in state.greetings.clone() {
        let first_line = greeting.text.lines().next().unwrap_or_default();
        let preview: String = first_line.chars().take(48).collect();
        let label = format!("[{}] {preview}", greeting.index);
        if ui
            .add_enabled(!state.greeting_inflight, egui::Button::new(label))
            .clicked()
        {
            state.push_action(SurfaceAction::SelectGreeting {
                index: greeting.index,
            });
        }
    }
    if !state.greeting_status.is_empty() {
        ui.colored_label(egui::Color32::LIGHT_RED, &state.greeting_status);
    }
}

pub fn show(ui: &mut egui::Ui, state: &mut SurfaceUiState, mic_active: bool) -> egui::Response {
    let output = ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
        ui.horizontal(|ui| {
            if ui.button(i18n::fl("chat-send")).clicked() {
                state.push_action(SurfaceAction::SendChat);
            }
            if ui
                .button(i18n::fl("chat-new-session"))
                .on_hover_text(i18n::fl("chat-new-session-hint"))
                .clicked()
            {
                state.push_action(SurfaceAction::NewSession);
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
                let rows = normalize_transcript(&state.history, &state.streaming_text);
                if rows.is_empty() {
                    render_greeting_picker(ui, state);
                } else {
                    for row in rows {
                        render_transcript_row(ui, row);
                    }
                }
            });

        response
    });

    output.inner
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(role: &str, text: &str) -> MessageResponse {
        MessageResponse {
            seq: 1,
            role: role.to_owned(),
            text: text.to_owned(),
        }
    }

    #[test]
    fn transcript_normalization_keeps_surface_roles_and_streaming_state() {
        let history = HistoryResponse {
            messages: vec![
                message("user", "hello"),
                message("assistant", "hi"),
                message("status", "model failed"),
                message("tool-summary", "searched"),
                message("inner", "private thought"),
            ],
            depth: "surface".to_owned(),
        };

        let rows = normalize_transcript(&history, "still writing");

        assert_eq!(rows.len(), 5);
        assert_eq!(
            rows[0],
            TranscriptRow {
                kind: TranscriptKind::User,
                state: TranscriptState::Stable,
                text: "hello",
            }
        );
        assert_eq!(rows[2].kind, TranscriptKind::Error);
        assert_eq!(rows[3].kind, TranscriptKind::Tool);
        assert_eq!(
            rows[4],
            TranscriptRow {
                kind: TranscriptKind::Assistant,
                state: TranscriptState::Streaming,
                text: "still writing",
            }
        );
    }

    #[test]
    fn transcript_normalization_hides_inner_and_thinking_rows() {
        let history = HistoryResponse {
            messages: vec![message("thinking", "private"), message("inner", "private")],
            depth: "surface".to_owned(),
        };

        assert!(normalize_transcript(&history, "").is_empty());
    }
}
