//! Chat panel for the surface viewport.

use crate::detail::chat_setup_gap;
use crate::i18n;
use crate::surface::{SurfaceAction, SurfaceUiState};
use ene_api::{HistoryResponse, MessageResponse};

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

/// Normalized conversation row: role, delivery state, and owned text. Overlay
/// and Chat share this model; it does not depend on a UI toolkit.
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

pub(crate) fn transcript_label(kind: TranscriptKind) -> String {
    i18n::fl(match kind {
        TranscriptKind::User => "chat-role-user",
        TranscriptKind::Assistant => "chat-role-assistant",
        TranscriptKind::Error => "chat-error",
        TranscriptKind::Tool => "chat-tool",
        TranscriptKind::System => "chat-system",
    })
}

/// A lone canonical greeting commits as soon as the picker renders; guard
/// against re-queueing while the selection is already pending or in flight.
pub(crate) fn request_single_greeting_commit(state: &mut SurfaceUiState) {
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

#[cfg(test)]
const COMPOSER_MIN_ROWS: usize = 3;
#[cfg(test)]
const COMPOSER_MAX_ROWS: usize = 8;
#[cfg(test)]
const COMPOSER_ROW_HEIGHT: f32 = 18.0;
#[cfg(test)]
const COMPOSER_VERTICAL_PADDING: f32 = 14.0;
#[cfg(test)]
const COMPOSER_MIN_HEIGHT: f32 = 64.0;

#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ComposerSendRequest {
    send: bool,
}

#[cfg(test)]
const COMPOSER_NONE: ComposerSendRequest = ComposerSendRequest { send: false };

#[cfg(test)]
const COMPOSER_SEND: ComposerSendRequest = ComposerSendRequest { send: true };

/// Rows the composer shows for the current draft: grows with content up to
/// the cap, past which the editor scrolls internally instead of pushing the
/// rest of the panel off screen.
#[cfg(test)]
#[must_use]
fn composer_metrics(draft: &str) -> (usize, f32) {
    let mut rows = draft.lines().count().max(1);
    if draft.ends_with('\n') {
        rows += 1;
    }
    rows = rows.clamp(COMPOSER_MIN_ROWS, COMPOSER_MAX_ROWS);
    #[expect(
        clippy::cast_precision_loss,
        reason = "row counts stay far below the f32 exact-integer range"
    )]
    let height = rows as f32 * COMPOSER_ROW_HEIGHT + COMPOSER_VERTICAL_PADDING;
    (rows, height.max(COMPOSER_MIN_HEIGHT))
}

#[must_use]
pub(crate) fn composer_send_allowed(state: &SurfaceUiState) -> bool {
    !state.chat_draft.trim().is_empty()
}

/// The multiline editor inserts the newline for the same Enter press that
/// requests the send; dropping it keeps blocked turns from collecting stray
/// blank lines at the end of the draft.
#[cfg(test)]
fn pop_enter_newline(draft: &mut String) {
    if draft.ends_with('\n') {
        draft.pop();
    }
}

#[cfg(test)]
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

/// The mic guard's Voice-setup button must not infer "STT missing" from
/// arbitrary status text; only the dedicated flag may show or arm it.
#[cfg(test)]
fn mic_cta_eligible(state: &SurfaceUiState, mic_active: bool) -> bool {
    state.stt_setup_needed && !mic_active
}

/// The chat-setup CTA may not piggyback on generic status text nor crowd out
/// live conversation rows or a visible greeting picker; only a dedicated setup
/// gap over a quiet panel may show or arm it. A lone greeting is committed
/// automatically and does not occupy the panel while that request is pending.
pub(crate) fn chat_setup_cta_eligible(state: &SurfaceUiState) -> bool {
    if chat_setup_gap(&state.chat_setup).is_none() || state.greetings.len() >= 2 {
        return false;
    }
    if !state.streaming_text.is_empty() {
        return false;
    }
    let rows = normalize_transcript(&state.history, &state.streaming_text);
    if rows.is_empty() {
        return true;
    }
    // A single committed bootstrap greeting (one assistant row, no user rows)
    // should still surface setup guidance until a real conversation starts.
    if rows.len() == 1
        && rows[0].role == TranscriptKind::Assistant
        && !state.history.messages.iter().any(|m| m.role == "user")
    {
        return true;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use ene_api::GreetingView;

    #[test]
    fn unrelated_status_never_arms_or_renders_the_mic_voice_cta() {
        let mut state = SurfaceUiState {
            // An unrelated failure path left text in the generic status line.
            status: "tool: execute: unknown skill skill".to_owned(),
            ..Default::default()
        };
        assert!(
            !mic_cta_eligible(&state, false),
            "unrelated status must never render or fire the Voice CTA"
        );

        // Only the mic toggle guard arms the flag; then the CTA is eligible.
        state.stt_setup_needed = true;
        assert!(mic_cta_eligible(&state, false));
        // With the mic claimed the guard no longer applies either.
        assert!(!mic_cta_eligible(&state, true));
    }

    #[test]
    fn chat_setup_cta_needs_a_gap_and_an_empty_transcript() {
        // Defaults leave chat unconfigured, so the bare surface arms the CTA.
        assert!(chat_setup_cta_eligible(&SurfaceUiState::default()));

        // Rows on screen displace the first-run CTA.
        let occupied = SurfaceUiState {
            history: HistoryResponse {
                messages: vec![message("user", "hello"), message("assistant", "hi")],
                depth: "surface".to_owned(),
            },
            ..Default::default()
        };
        assert!(!chat_setup_cta_eligible(&occupied));

        // A visible greeting picker displaces it even before any row exists.
        let greeting_pending = SurfaceUiState {
            greetings: vec![greeting(0, "Hi!"), greeting(1, "Hello!")],
            history: HistoryResponse {
                messages: Vec::new(),
                depth: "surface".to_owned(),
            },
            ..Default::default()
        };
        assert!(!chat_setup_cta_eligible(&greeting_pending));

        // A lone greeting is committed automatically and leaves the panel
        // free for setup guidance until the assistant row arrives.
        let single_greeting = SurfaceUiState {
            greetings: vec![greeting(0, "Hi!")],
            ..Default::default()
        };
        assert!(chat_setup_cta_eligible(&single_greeting));

        // A stream in flight displaces it even without stable rows.
        let streaming = SurfaceUiState {
            streaming_text: "still writing".to_owned(),
            ..Default::default()
        };
        assert!(!chat_setup_cta_eligible(&streaming));

        // Configuring a model resolves the gap and removes the CTA.
        let mut configured = SurfaceUiState::default();
        configured.chat_setup.chat_plugin = "provider.gguf".to_owned();
        configured.chat_setup.chat_model = "local-model".to_owned();
        assert!(!chat_setup_cta_eligible(&configured));

        // Arbitrary generic status text neither arms nor gates the signal.
        assert!(chat_setup_cta_eligible(&SurfaceUiState {
            status: "tool: execute: unknown skill skill".to_owned(),
            ..Default::default()
        }));

        // A single committed bootstrap greeting in history still shows the
        // setup CTA until the user sends a real message.
        let bootstrap_committed = SurfaceUiState {
            history: HistoryResponse {
                messages: vec![message("assistant", "Hello, I am Alicia.")],
                depth: "surface".to_owned(),
            },
            ..Default::default()
        };
        assert!(chat_setup_cta_eligible(&bootstrap_committed));

        // Once the user has spoken, even a single assistant row suppresses it.
        let with_user = SurfaceUiState {
            history: HistoryResponse {
                messages: vec![
                    message("user", "hi"),
                    message("assistant", "Hello, I am Alicia."),
                ],
                depth: "surface".to_owned(),
            },
            ..Default::default()
        };
        assert!(!chat_setup_cta_eligible(&with_user));
    }

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
    fn composer_height_grows_with_content_and_caps() {
        let (min_rows, min_height) = composer_metrics("");
        assert_eq!(min_rows, COMPOSER_MIN_ROWS);
        assert!(min_height >= COMPOSER_MIN_HEIGHT);

        let grown_draft = "one\n".repeat(COMPOSER_MAX_ROWS);
        let (_, grown) = composer_metrics(grown_draft.trim_end());
        assert!(grown > min_height);

        let long = "line\n".repeat(COMPOSER_MAX_ROWS * 2);
        let (capped_rows, capped_height) = composer_metrics(&long);
        assert_eq!(capped_rows, COMPOSER_MAX_ROWS);
        let (_, saturated_height) = composer_metrics("a\nb\nc\nd\ne\nf\ng\nh");
        assert!((capped_height - saturated_height).abs() < f32::EPSILON);
    }

    #[test]
    fn whitespace_draft_blocks_send_but_keeps_typing() {
        let mut state = SurfaceUiState {
            chat_draft: "   \n\t ".to_owned(),
            ..Default::default()
        };

        assert!(!composer_send_allowed(&state));
        state.chat_draft = "real words".to_owned();
        assert!(composer_send_allowed(&state));
    }

    #[test]
    fn enter_newline_is_removed_before_sending() {
        let mut draft = "hello\n".to_owned();
        pop_enter_newline(&mut draft);
        assert_eq!(draft, "hello");

        pop_enter_newline(&mut draft);
        assert_eq!(
            draft, "hello",
            "only one trailing newline is removed per send"
        );
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

    #[test]
    fn ime_preedit_blocks_send_even_when_enter_arrives_first() {
        assert_eq!(
            composer_request_for_key(true, false, true),
            COMPOSER_NONE,
            "active IME preedit must claim Enter for the composition"
        );
    }
}
