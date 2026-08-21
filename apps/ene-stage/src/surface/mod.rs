//! Surface chrome: chat, caption, spotlight, approvals.

mod approvals;
mod caption;
mod chat;
mod spotlight;

use ene_api::{HistoryResponse, MessageMode};

use crate::detail::DetailTab;
use crate::i18n;

pub use approvals::PendingApproval;
pub use spotlight::SpotlightAction;

pub const CHAT_WINDOW_WIDTH: u32 = 520;
pub const CHAT_WINDOW_HEIGHT: u32 = 560;

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
}

pub fn show_chat(ui: &mut egui::Ui, state: &mut SurfaceUiState, mic_active: bool) {
    chat::show(ui, state, mic_active);
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
}

pub fn show_caption(ctx: &egui::Context, state: &SurfaceUiState, font_size: f32) {
    caption::show(ctx, state, font_size);
}

pub fn show_spotlight(ctx: &egui::Context, state: &mut SurfaceUiState) -> Option<SpotlightAction> {
    let action = spotlight::show(ctx);
    if action.is_some() {
        state.spotlight_open = false;
    }
    action
}
