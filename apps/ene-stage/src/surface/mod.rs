//! Surface chrome: chat, caption, spotlight, approvals.

mod approvals;
pub(crate) mod caption;
mod chat;
mod spotlight;

use ene_api::{HistoryResponse, MessageMode};

use crate::detail::DetailTab;
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
    BargeIn,
    CancelTurn,
    ToggleMic,
    Approval { decision: String },
    AnswerQuestion,
    OpenDetail(DetailTab),
    Quit,
    PersistCharacterPos,
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
    pub history: HistoryResponse,
    pub streaming_text: String,
    pub caption: String,
    pub pending_approval: Option<PendingApproval>,
    pub pending_question: Option<PendingQuestion>,
    pub spotlight_open: bool,
    pub chat_open: bool,
    pub caption_open: bool,
    pub status: String,
    pub voice_state: String,
    pub exclusive_notice: String,
    pub quit: bool,
    pub character_pos: [f32; 2],
    pub dragging_character: bool,
    pub pending_actions: Vec<SurfaceAction>,
    pub message_mode: MessageMode,
    pub turn_active: bool,
}

impl Default for SurfaceUiState {
    fn default() -> Self {
        Self {
            chat_draft: String::new(),
            focus_chat: false,
            chat_input_focused: false,
            history: HistoryResponse {
                messages: Vec::new(),
                depth: "surface".to_owned(),
            },
            streaming_text: String::new(),
            caption: String::new(),
            pending_approval: None,
            pending_question: None,
            spotlight_open: false,
            chat_open: true,
            caption_open: false,
            status: i18n::fl("status-ready"),
            voice_state: String::new(),
            exclusive_notice: String::new(),
            quit: false,
            character_pos: [0.78, 0.5],
            dragging_character: false,
            pending_actions: Vec::new(),
            message_mode: MessageMode::Prompt,
            turn_active: false,
        }
    }
}

impl SurfaceUiState {
    pub fn push_action(&mut self, action: SurfaceAction) {
        self.pending_actions.push(action);
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
            .memory_mut(|mem| mem.request_focus(egui::Id::new("stage-chat-input")));
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
    let action = spotlight::show(ctx);
    if action.is_some() {
        state.spotlight_open = false;
    }
    action
}

#[cfg(test)]
mod tests {
    use super::SurfaceUiState;

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
}
