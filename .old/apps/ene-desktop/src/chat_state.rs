use std::fmt::Write as _;

use crate::settings::{PendingPermission, PendingUserInput, QuestionDraft};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    User,
    Assistant,
}

impl Role {
    #[must_use]
    pub fn from_api(role: &str) -> Option<Self> {
        match role {
            "user" => Some(Self::User),
            "assistant" => Some(Self::Assistant),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HistoryEntry {
    pub role: Role,
    pub content: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChatMessage {
    pub role: Role,
    pub content: String,
    pub is_streaming: bool,
}

#[derive(Clone, Debug)]
pub struct ChatState {
    pub chat_window_visible: bool,
    pub input_draft: String,
    pub messages: Vec<ChatMessage>,
    pub scroll_to_bottom: bool,
    pub needs_history_reconcile: bool,
    pub pending_permission: Option<PendingPermission>,
    pub pending_user_input: Option<PendingUserInput>,
    pub user_input_drafts: Vec<QuestionDraft>,
    pub undo_status: Option<String>,
    pub greeting_status: Option<String>,
}

impl ChatState {
    #[cfg(test)]
    pub fn new() -> Self {
        Self {
            chat_window_visible: true,
            ..Self::default()
        }
    }

    pub fn sync_from_history(&mut self, history: &[HistoryEntry]) {
        self.messages = history
            .iter()
            .map(|entry| ChatMessage {
                role: entry.role,
                content: entry.content.clone(),
                is_streaming: false,
            })
            .collect();
        self.scroll_to_bottom = true;
        self.needs_history_reconcile = false;
        self.greeting_status = None;
    }

    pub fn append_text_delta(&mut self, delta: &str) {
        if let Some(last) = self.messages.last_mut()
            && last.role == Role::Assistant
            && last.is_streaming
        {
            last.content.push_str(delta);
            self.scroll_to_bottom = true;
            return;
        }
        self.messages.push(ChatMessage {
            role: Role::Assistant,
            content: delta.to_string(),
            is_streaming: true,
        });
        self.scroll_to_bottom = true;
    }

    pub fn finish_streaming(&mut self) {
        if let Some(last) = self.messages.last_mut() {
            last.is_streaming = false;
        }
        self.needs_history_reconcile = true;
    }

    pub fn finish_streaming_with_error(&mut self, message: &str) {
        let prefix = i18n_embed_fl::fl!(crate::i18n::loader(), "chat-error-prefix");
        let labeled = format!("[{prefix}] {message}");
        if let Some(last) = self.messages.last_mut()
            && last.role == Role::Assistant
            && last.is_streaming
        {
            if last.content.is_empty() {
                last.content = labeled;
            } else {
                #[expect(
                    clippy::let_underscore_must_use,
                    reason = "fmt::Write to a String is infallible in practice"
                )]
                let _ = write!(last.content, "\n{labeled}");
            }
            last.is_streaming = false;
        } else {
            self.messages.push(ChatMessage {
                role: Role::Assistant,
                content: labeled,
                is_streaming: false,
            });
        }
        self.scroll_to_bottom = true;
        self.needs_history_reconcile = true;
    }

    pub fn prepare_send(&mut self, message: &str) -> bool {
        let trimmed = message.trim();
        if trimmed.is_empty() {
            return false;
        }
        self.messages.push(ChatMessage {
            role: Role::User,
            content: trimmed.to_string(),
            is_streaming: false,
        });
        self.messages.push(ChatMessage {
            role: Role::Assistant,
            content: String::new(),
            is_streaming: true,
        });
        self.input_draft.clear();
        self.scroll_to_bottom = true;
        true
    }
}

impl Default for ChatState {
    fn default() -> Self {
        Self {
            chat_window_visible: true,
            input_draft: String::new(),
            messages: Vec::new(),
            scroll_to_bottom: false,
            needs_history_reconcile: false,
            pending_permission: None,
            pending_user_input: None,
            user_input_drafts: Vec::new(),
            undo_status: None,
            greeting_status: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prepare_send_appends_user_and_streaming_assistant() {
        let mut state = ChatState::new();
        assert!(state.prepare_send("hello"));
        assert_eq!(state.messages[0].role, Role::User);
        assert_eq!(state.messages[1].role, Role::Assistant);
        assert!(state.messages[1].is_streaming);
    }

    #[test]
    fn sync_from_history_keeps_surface_roles() {
        let mut state = ChatState::new();
        state.sync_from_history(&[
            HistoryEntry {
                role: Role::User,
                content: "hi".to_owned(),
            },
            HistoryEntry {
                role: Role::Assistant,
                content: "hello".to_owned(),
            },
        ]);
        assert_eq!(state.messages.len(), 2);
        assert!(!state.needs_history_reconcile);
    }
}
