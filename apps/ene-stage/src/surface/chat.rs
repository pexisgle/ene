//! Chat panel for the surface viewport.

use crate::detail::{DetailTab, chat_setup_gap, chat_setup_status};
use crate::i18n;
use crate::surface::{SurfaceAction, SurfaceUiState};
use ene_api::{HistoryResponse, MessageMode, MessageResponse};

/// Role a transcript row plays in the conversation view. The kind decides
/// alignment and the visible label, so meaning never rests on color alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TranscriptKind {
    User,
    Assistant,
    Error,
    Tool,
    System,
}

/// Delivery state of a row. Streaming rows get the caret suffix and the
/// waiting placeholder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TranscriptState {
    Stable,
    Error,
    Streaming,
}

/// Normalized conversation row: role, delivery state, and owned text. Kept
/// independent of egui so follow-up stage features can reuse the transcript.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ChatMessageView {
    pub(crate) role: TranscriptKind,
    pub(crate) state: TranscriptState,
    pub(crate) text: String,
}

pub(crate) fn normalize_transcript(
    history: &HistoryResponse,
    streaming_text: &str,
) -> Vec<ChatMessageView> {
    let mut rows = history
        .messages
        .iter()
        .filter_map(normalize_message)
        .collect::<Vec<_>>();
    if !streaming_text.is_empty() {
        rows.push(ChatMessageView {
            role: TranscriptKind::Assistant,
            state: TranscriptState::Streaming,
            text: streaming_text.to_owned(),
        });
    }
    rows
}

fn normalize_message(message: &MessageResponse) -> Option<ChatMessageView> {
    let (kind, state) = match message.role.as_str() {
        "user" => (TranscriptKind::User, TranscriptState::Stable),
        "assistant" => (TranscriptKind::Assistant, TranscriptState::Stable),
        "status" | "error" => (TranscriptKind::Error, TranscriptState::Error),
        "tool" | "tool-summary" => (TranscriptKind::Tool, TranscriptState::Stable),
        "inner" | "thinking" => return None,
        _ => (TranscriptKind::System, TranscriptState::Stable),
    };
    Some(ChatMessageView {
        role: kind,
        state,
        text: message.text.clone(),
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

pub(crate) fn render_message_bubble(ui: &mut egui::Ui, row: &ChatMessageView) {
    let is_user = row.role == TranscriptKind::User;
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
                egui::RichText::new(transcript_label(row.role))
                    .small()
                    .weak(),
            );
            let mut text = row.text.clone();
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
    if state.greetings.len() == 1 {
        request_single_greeting_commit(state);
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

/// A lone canonical greeting commits as soon as the picker renders; guard
/// against re-queueing while the selection is already pending or in flight.
fn request_single_greeting_commit(state: &mut SurfaceUiState) {
    let Some(greeting) = state.greetings.first() else {
        return;
    };
    if state.greeting_inflight
        || state
            .pending_actions
            .iter()
            .any(|action| matches!(action, SurfaceAction::SelectGreeting { .. }))
    {
        return;
    }
    state.push_action(SurfaceAction::SelectGreeting {
        index: greeting.index,
    });
}

pub(crate) const CHAT_INPUT_ID: &str = "stage-chat-input";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ComposerSendRequest {
    send: bool,
}

const COMPOSER_NONE: ComposerSendRequest = ComposerSendRequest { send: false };

const COMPOSER_SEND: ComposerSendRequest = ComposerSendRequest { send: true };

#[must_use]
fn composer_request_for_key(
    enter_pressed: bool,
    shift_pressed: bool,
    composing: bool,
) -> ComposerSendRequest {
    if !enter_pressed || shift_pressed || composing {
        return COMPOSER_NONE;
    }
    COMPOSER_SEND
}

#[must_use]
fn composer_send_requested(ui: &egui::Ui) -> ComposerSendRequest {
    ui.input(|input| {
        let enter_pressed = input.events.iter().any(|event| {
            matches!(
                event,
                egui::Event::Key {
                    key: egui::Key::Enter,
                    pressed: true,
                    ..
                }
            )
        });
        let composing = input.events.iter().any(|event| {
            matches!(
                event,
                egui::Event::Ime(egui::ImeEvent::Preedit { text, .. }) if !text.is_empty()
            )
        });
        composer_request_for_key(enter_pressed, input.modifiers.shift, composing)
    })
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
        ui.collapsing(i18n::fl("chat-overlay-hint"), |ui| {
            ui.label(i18n::fl("chat-overlay-hint"));
        });
        if !state.exclusive_notice.is_empty() {
            ui.colored_label(egui::Color32::YELLOW, &state.exclusive_notice);
        }

        let response = ui.add(
            egui::TextEdit::multiline(&mut state.chat_draft)
                .id_salt(CHAT_INPUT_ID)
                .hint_text(i18n::fl("chat-placeholder"))
                .desired_width(ui.available_width())
                .desired_rows(2)
                .min_size(egui::vec2(ui.available_width(), 56.0))
                .return_key(Some(egui::KeyboardShortcut::new(
                    egui::Modifiers::SHIFT,
                    egui::Key::Enter,
                )))
                .code_editor(),
        );
        let send_request = response.has_focus().then(|| composer_send_requested(ui));
        if send_request.as_ref().is_some_and(|request| request.send)
            && !state.chat_draft.trim().is_empty()
        {
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
                        render_message_bubble(ui, &row);
                    }
                }
                if let Some(gap) = chat_setup_gap(&state.chat_setup) {
                    ui.add_space(4.0);
                    ui.weak(chat_setup_status(gap));
                }
            });

        response
    });

    output.inner
}

#[cfg(test)]
mod tests {
    use super::*;
    use ene_api::GreetingView;

    fn message(role: &str, text: &str) -> MessageResponse {
        MessageResponse {
            seq: 1,
            role: role.to_owned(),
            text: text.to_owned(),
        }
    }

    fn greeting(index: u32, text: &str) -> GreetingView {
        GreetingView {
            index,
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
            ChatMessageView {
                role: TranscriptKind::User,
                state: TranscriptState::Stable,
                text: "hello".to_owned(),
            }
        );
        assert_eq!(rows[2].role, TranscriptKind::Error);
        assert_eq!(rows[2].state, TranscriptState::Error);
        assert_eq!(rows[3].role, TranscriptKind::Tool);
        assert_eq!(
            rows[4],
            ChatMessageView {
                role: TranscriptKind::Assistant,
                state: TranscriptState::Streaming,
                text: "still writing".to_owned(),
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

    #[test]
    fn greeting_picker_without_greetings_shows_empty_state() {
        let mut state = SurfaceUiState {
            greetings: Vec::new(),
            ..Default::default()
        };

        request_single_greeting_commit(&mut state);

        assert!(state.pending_actions.is_empty());
    }

    #[test]
    fn single_greeting_commits_once_without_click() {
        let mut state = SurfaceUiState {
            greetings: vec![greeting(0, "Welcome back.")],
            ..Default::default()
        };
        state.push_action(SurfaceAction::SelectGreeting { index: 0 });

        request_single_greeting_commit(&mut state);

        assert_eq!(state.pending_actions.len(), 1);

        state.greeting_inflight = true;
        request_single_greeting_commit(&mut state);

        assert_eq!(state.pending_actions.len(), 1);
    }

    #[test]
    fn multiple_greetings_wait_for_explicit_selection() {
        let state = SurfaceUiState {
            greetings: vec![
                greeting(0, "First greeting."),
                greeting(1, "Second greeting."),
            ],
            ..Default::default()
        };

        assert!(state.greetings.len() > 1, "picker must wait for a click");

        assert!(SurfaceUiState::default().pending_actions.is_empty());
    }

    #[test]
    fn existing_history_suppresses_greeting_picker() {
        let mut state = SurfaceUiState::default();
        state.history.messages = vec![message("assistant", "hello")];

        let rows = normalize_transcript(&state.history, "");

        assert!(!rows.is_empty(), "existing history must hide the picker");
    }

    #[test]
    fn composer_contract_keeps_shift_enter_out_of_send_path() {
        assert_eq!(composer_request_for_key(true, false, false), COMPOSER_SEND);
        assert_eq!(composer_request_for_key(true, true, false), COMPOSER_NONE);
        assert_eq!(composer_request_for_key(true, false, true), COMPOSER_NONE);
    }

    #[test]
    fn multiline_draft_preserves_paste_newlines() {
        let state = SurfaceUiState {
            chat_draft: "first\nsecond\n".to_owned(),
            ..Default::default()
        };

        assert_eq!(state.chat_draft.lines().count(), 2);
        assert!(state.chat_draft.ends_with('\n'));
    }

    #[test]
    fn send_requires_non_whitespace_draft() {
        let state = SurfaceUiState {
            chat_draft: "  \n\t".to_owned(),
            ..Default::default()
        };

        assert!(state.chat_draft.trim().is_empty());
    }
}
