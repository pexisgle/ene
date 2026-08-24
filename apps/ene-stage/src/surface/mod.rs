//! Surface chrome: chat, caption, spotlight, approvals.

mod approvals;
pub(crate) mod caption;
mod chat;
mod spotlight;

use std::collections::BTreeMap;

use ene_api::{GreetingView, HistoryResponse, MessageMode};

use crate::detail::DetailTab;
use crate::detail::DetailUiState;
use crate::i18n;

pub use approvals::PendingApproval;
pub use spotlight::SpotlightAction;

pub(crate) const CHAT_WINDOW_WIDTH: u32 = 520;
pub(crate) const CHAT_WINDOW_HEIGHT: u32 = 560;

const _: () = {
    assert!(CHAT_WINDOW_WIDTH >= 480);
    assert!(CHAT_WINDOW_HEIGHT >= 560);
};

#[derive(Debug, Clone)]
pub enum SurfaceAction {
    SendChat,
    NewSession,
    SelectGreeting { index: u32 },
    BargeIn,
    CancelTurn,
    ToggleMic,
    Approval { id: String, decision: String },
    AnswerQuestion,
    OpenDetail(DetailTab),
    Quit,
    PersistBodyPosition { soul_id: String },
}

#[derive(Debug, Clone)]
pub struct PendingQuestion {
    pub id: String,
    pub prompt: String,
}

#[derive(Debug, Clone)]
pub struct SurfaceUiState {
    pub chat_draft: String,
    pub focus_chat: bool,
    pub chat_input_focused: bool,
    /// Mirrors the Detail window's chat readiness so the chat surface can
    /// show setup guidance without polling core settings itself.
    pub chat_setup: DetailUiState,
    pub history: HistoryResponse,
    pub greetings: Vec<GreetingView>,
    pub greeting_inflight: bool,
    pub greeting_status: String,
    pub streaming_text: String,
    pub caption: String,
    pub pending_approval: Option<PendingApproval>,
    pub pending_question: Option<PendingQuestion>,
    pub spotlight_open: bool,
    pub spotlight_query: String,
    pub spotlight_selected: usize,
    pub spotlight_hotkey_ok: bool,
    pub chat_open: bool,
    pub caption_open: bool,
    pub status: String,
    pub voice_state: String,
    pub exclusive_notice: String,
    pub quit: bool,
    pub positions: BTreeMap<String, [f32; 2]>,
    pub drag: Option<crate::drag::BodyDrag>,
    pub hover_soul: Option<String>,
    pub pending_actions: Vec<SurfaceAction>,
    pub(crate) new_session_inflight: bool,
    pub message_mode: MessageMode,
    pub turn_active: bool,
}

impl Default for SurfaceUiState {
    fn default() -> Self {
        Self {
            chat_draft: String::new(),
            focus_chat: false,
            chat_input_focused: false,
            chat_setup: DetailUiState::default(),
            history: HistoryResponse {
                messages: Vec::new(),
                depth: "surface".to_owned(),
            },
            greetings: Vec::new(),
            greeting_inflight: false,
            greeting_status: String::new(),
            streaming_text: String::new(),
            caption: String::new(),
            pending_approval: None,
            pending_question: None,
            spotlight_open: false,
            spotlight_query: String::new(),
            spotlight_selected: 0,
            spotlight_hotkey_ok: false,
            chat_open: true,
            caption_open: false,
            status: i18n::fl("status-ready"),
            voice_state: String::new(),
            exclusive_notice: String::new(),
            quit: false,
            positions: BTreeMap::new(),
            drag: None,
            hover_soul: None,
            pending_actions: Vec::new(),
            new_session_inflight: false,
            message_mode: MessageMode::Prompt,
            turn_active: false,
        }
    }
}

impl SurfaceUiState {
    pub fn push_action(&mut self, action: SurfaceAction) {
        if matches!(action, SurfaceAction::NewSession) && self.new_session_inflight {
            return;
        }
        self.pending_actions.push(action);
    }

    /// Closing the Chat window removes the redraw that would normally clear
    /// input focus, so the flag must be reset alongside `chat_open`.
    pub fn close_chat(&mut self) {
        self.chat_open = false;
        self.chat_input_focused = false;
    }

    /// Whether a chat window should currently be kept alive.
    #[must_use]
    pub fn chat_window_exists(&self) -> bool {
        self.chat_open
    }

    /// A new turn supersedes the previous turn's composer feedback.
    pub(crate) fn begin_send(&mut self) {
        self.status.clear();
        self.turn_active = true;
    }

    pub(crate) fn apply_text_delta(&mut self, text: &str, captions: bool) {
        if caption::is_speech_caption(text) {
            self.streaming_text.push_str(text);
            if captions {
                self.caption.clone_from(&self.streaming_text);
                self.caption_open = true;
            }
            return;
        }
        if !text.trim().is_empty() {
            text.clone_into(&mut self.status);
            self.dismiss_caption();
        }
    }

    pub(crate) fn on_turn_ended(&mut self) {
        self.streaming_text.clear();
        self.turn_active = false;
        self.status.clear();
        self.dismiss_caption();
    }

    pub(crate) fn dismiss_caption(&mut self) {
        self.caption.clear();
        self.caption_open = false;
    }

    #[must_use]
    pub(crate) fn caption_visible(&self) -> bool {
        self.caption_open && caption::is_speech_caption(&self.caption)
    }
}

pub fn show_chat(ui: &mut egui::Ui, state: &mut SurfaceUiState, mic_active: bool) {
    let response = chat::show(ui, state, mic_active);
    if state.pending_approval.is_some() {
        approvals::show(ui.ctx(), state);
    }
    if let Some(question) = state.pending_question.clone() {
        egui::Window::new(i18n::fl("ask-user-title"))
            .collapsible(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 40.0])
            .show(ui.ctx(), |ui| {
                ui.label(&question.prompt);
                if ui.button(i18n::fl("ask-user-answer")).clicked() {
                    state.push_action(SurfaceAction::AnswerQuestion);
                }
            });
    }
    if state.focus_chat {
        state.focus_chat = false;
        ui.ctx()
            .memory_mut(|mem| mem.request_focus(egui::Id::new(chat::CHAT_INPUT_ID)));
    }
    state.chat_input_focused = response.has_focus();
}

pub fn show_caption(
    ctx: &egui::Context,
    state: &SurfaceUiState,
    font_size: f32,
    position: &str,
    pinned: bool,
) {
    caption::show(ctx, state, font_size, position, pinned);
}

pub fn show_spotlight(ctx: &egui::Context, state: &mut SurfaceUiState) -> Option<SpotlightAction> {
    let action = spotlight::show(ctx, state);
    if action.is_some() {
        state.spotlight_open = false;
        state.spotlight_query.clear();
        state.spotlight_selected = 0;
    }
    action
}

#[cfg(test)]
mod tests {
    use super::{SurfaceAction, SurfaceUiState};

    #[test]
    fn a_new_send_clears_the_previous_turn_status() {
        let mut state = SurfaceUiState {
            status: "tool: execute: unknown skill skill".to_owned(),
            ..Default::default()
        };

        state.begin_send();

        assert!(state.status.is_empty());
        assert!(state.turn_active);
    }

    #[test]
    fn ending_a_turn_clears_progress_and_terminal_composer_feedback() {
        let mut state = SurfaceUiState::default();
        state.apply_text_delta("Hello from the companion.", false);

        state.on_turn_ended();

        assert!(state.status.is_empty());
        assert!(!state.turn_active);
    }

    #[test]
    fn provider_error_delta_goes_to_status_not_caption() {
        let mut state = SurfaceUiState::default();
        state.apply_text_delta(
            "The chat provider failed: model: call failed: 401 Unauthorized",
            true,
        );
        assert!(!state.caption_visible());
        assert!(state.caption.is_empty());
        assert!(state.streaming_text.is_empty());
        assert!(state.status.contains("401 Unauthorized"));
    }

    #[test]
    fn speech_caption_closes_when_the_turn_ends() {
        let mut state = SurfaceUiState::default();
        state.apply_text_delta("Hello from the companion.", true);
        assert!(state.caption_visible());
        assert_eq!(state.caption, "Hello from the companion.");
        state.on_turn_ended();
        assert!(!state.caption_visible());
        assert!(state.caption.is_empty());
        assert!(state.streaming_text.is_empty());
        state.apply_text_delta("Next turn.", true);
        assert!(state.caption_visible());
        assert_eq!(state.caption, "Next turn.");
    }

    #[test]
    fn closing_chat_clears_stale_input_focus() {
        let mut state = SurfaceUiState::default();
        assert!(state.chat_open);
        state.chat_input_focused = true;

        state.close_chat();

        assert!(!state.chat_open);
        assert!(
            !state.chat_input_focused,
            "overlay shortcuts must not stay blocked after Chat closes"
        );
    }

    #[test]
    fn new_chat_actions_are_deduplicated_while_inflight() {
        let mut state = SurfaceUiState::default();
        state.push_action(SurfaceAction::NewSession);
        state.new_session_inflight = true;
        state.push_action(SurfaceAction::NewSession);

        let actions: Vec<_> = state
            .pending_actions
            .iter()
            .filter(|action| matches!(action, SurfaceAction::NewSession))
            .collect();
        assert_eq!(actions.len(), 1);
    }
}
