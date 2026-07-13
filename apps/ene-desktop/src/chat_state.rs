//! Chat window state: message list, input draft, and in-flight dialogs.

use ene_runtime::{HistoryEntry, Role};

use crate::settings::{PendingPermission, PendingUserInput, QuestionDraft};

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
            .filter(|entry| entry.role != Role::System)
            .map(|entry| ChatMessage {
                role: entry.role,
                content: entry.content.clone(),
                is_streaming: false,
            })
            .collect();
        self.scroll_to_bottom = true;
        self.needs_history_reconcile = false;
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

    /// End a failed stream, optionally appending an error note to the
    /// in-flight assistant bubble so the user sees why processing stopped.
    pub fn finish_streaming_with_error(&mut self, message: &str) {
        if let Some(last) = self.messages.last_mut()
            && last.role == Role::Assistant
            && last.is_streaming
        {
            if last.content.is_empty() {
                last.content = format!("[Error] {message}");
            } else {
                last.content.push_str(&format!("\n[Error] {message}"));
            }
            last.is_streaming = false;
        } else {
            self.messages.push(ChatMessage {
                role: Role::Assistant,
                content: format!("[Error] {message}"),
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
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prepare_send_rejects_empty_input() {
        let mut state = ChatState::default();
        assert!(!state.prepare_send("   "));
        assert!(state.messages.is_empty());
    }

    #[test]
    fn prepare_send_adds_user_and_streaming_assistant() {
        let mut state = ChatState::default();
        assert!(state.prepare_send("hello"));
        assert_eq!(state.messages.len(), 2);
        assert_eq!(state.messages[0].role, Role::User);
        assert_eq!(state.messages[0].content, "hello");
        assert!(state.messages[1].is_streaming);
        assert!(state.input_draft.is_empty());
    }

    #[test]
    fn append_text_delta_extends_streaming_assistant() {
        let mut state = ChatState::default();
        state.prepare_send("hi");
        state.append_text_delta("there");
        assert_eq!(state.messages[1].content, "there");
    }

    #[test]
    fn sync_from_history_skips_system_messages() {
        let mut state = ChatState::default();
        state.sync_from_history(&[
            HistoryEntry {
                role: Role::System,
                content: "hidden".into(),
            },
            HistoryEntry {
                role: Role::User,
                content: "hi".into(),
            },
            HistoryEntry {
                role: Role::Assistant,
                content: "hello".into(),
            },
        ]);
        assert_eq!(state.messages.len(), 2);
        assert_eq!(state.messages[0].content, "hi");
    }

    #[test]
    fn finish_streaming_clears_flag_and_marks_reconcile() {
        let mut state = ChatState::default();
        state.prepare_send("x");
        state.finish_streaming();
        assert!(!state.messages[1].is_streaming);
        assert!(state.needs_history_reconcile);
    }
}
