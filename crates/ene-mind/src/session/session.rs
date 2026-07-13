use super::error::EneSessionError;
use super::session_split::{
    PendingSplitTask, SplitResult, SplitTaskInput, generate_session_id, poll_split_result,
};
use super::special_token::split_text_and_special_tokens;
use super::types::{CardName, SessionId};
use chrono::{DateTime, Utc};
use ene_ai::EmbeddingProvider;
use ene_ai::Role;
use ene_config::{CharacterCardV3, EneConfig, ResolvedExpression, resolve_expressions};

use crate::lifecycle::HistoryEntry;
use ene_store::MemoryStore;
use std::borrow::Cow;
use std::sync::Arc;

/// Manages the conversation history with automatic trimming.
#[derive(Clone, Debug)]
pub struct ConversationHistory {
    /// Ordered history entries.
    pub conversation_history: Vec<HistoryEntry>,
    /// Maximum number of turns to retain.
    pub max_history_turns: usize,
}

impl ConversationHistory {
    fn trim_history(&mut self) {
        let max = self.max_history_turns * 2;
        if self.conversation_history.len() > max {
            let excess = self.conversation_history.len() - max;
            self.conversation_history.drain(0..excess);
        }
    }
}

/// Holds the current display buffer and partial token carry-over.
#[derive(Clone, Debug, Default)]
pub struct DisplayState {
    /// Accumulated display text for the current response.
    pub display_buffer: String,
    /// Partial token text carried from a previous chunk.
    pub token_carry: String,
}

/// Context for the memory subsystem within a session.
#[derive(Clone)]
pub struct MemoryContext {
    /// Optional memory store.
    pub memory_store: Option<Arc<MemoryStore>>,
    /// Optional embedding provider.
    pub embedding_provider: Option<Arc<dyn EmbeddingProvider>>,
    /// The current session ID.
    pub session_id: SessionId,
    /// Timestamp when the session started.
    pub session_started_at: chrono::DateTime<chrono::Utc>,
    /// Embedding of the pending user input.
    pub pending_embedding: Option<Vec<f32>>,
    /// Cached hash of the last synced CCv3 character memory index.
    pub ccv3_memory_hash: Option<u64>,
}

/// Tracks session metadata like embedding and timing.
#[derive(Clone, Debug, Default)]
pub struct SessionState {
    /// The embedding of the last user input.
    pub last_input_embedding: Option<Vec<f32>>,
    /// Timestamp of the last received message.
    pub last_message_time: Option<DateTime<Utc>>,
    /// The current conversation turn count.
    pub current_turn_count: usize,
    /// When a session split is in flight, the length of `conversation_history`
    /// at the moment the snapshot was taken. The actor uses this to apply the
    /// split result: it drains everything before the last snapshot entry so
    /// that the triggering turn and any subsequent user/assistant messages
    /// added while the split was running are preserved.
    pub pending_split_snapshot_len: Option<usize>,
    /// Last resolved expression name (in-session hysteresis).
    pub last_resolved_expression: String,
    /// When the last expression change occurred.
    pub last_expression_changed_at: Option<DateTime<Utc>>,
}

/// Central session container holding conversation history, display state, memory context,
/// and the loaded character card. Shared between the streaming engine and the CLI/GUI frontends.
#[derive(Clone)]
pub struct ConversationSession {
    /// Conversation history state.
    pub(crate) history: ConversationHistory,
    /// Display buffer state.
    pub display: DisplayState,
    /// Memory context state.
    pub memory: MemoryContext,
    /// Session metadata state.
    pub(crate) state: SessionState,
    /// The loaded character card.
    pub character_card: Option<CharacterCardV3>,
    /// The filesystem path to the current character card.
    current_card_path: String,
}

impl std::fmt::Debug for ConversationSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConversationSession")
            .field(
                "conversation_history_len",
                &self.history.conversation_history.len(),
            )
            .field("max_history_turns", &self.history.max_history_turns)
            .field("current_card_path", &self.current_card_path)
            .field("memory_enabled", &self.memory.memory_store.is_some())
            .field("session_id", &self.memory.session_id)
            .field("turn_count", &self.state.current_turn_count)
            .finish()
    }
}

impl Default for ConversationSession {
    fn default() -> Self {
        Self::new()
    }
}

impl ConversationSession {
    /// Creates a new empty session with a fresh session ID and zero turn count.
    #[must_use]
    pub fn new() -> Self {
        Self {
            history: ConversationHistory {
                conversation_history: Vec::new(),
                max_history_turns: 20, // overridden by SessionConfig on first run
            },
            display: DisplayState {
                display_buffer: String::new(),
                token_carry: String::new(),
            },
            memory: MemoryContext {
                memory_store: None,
                embedding_provider: None,
                session_id: generate_session_id(),
                session_started_at: chrono::Utc::now(),
                pending_embedding: None,
                ccv3_memory_hash: None,
            },
            state: SessionState {
                last_input_embedding: None,
                last_message_time: None,
                current_turn_count: 0,
                pending_split_snapshot_len: None,
                last_resolved_expression: String::new(),
                last_expression_changed_at: None,
            },
            character_card: None,
            current_card_path: String::new(),
        }
    }

    /// Attaches a memory store and embedding provider for long-term memory.
    pub fn init_memory(&mut self, store: Arc<MemoryStore>, embedder: Arc<dyn EmbeddingProvider>) {
        self.memory.memory_store = Some(store);
        self.memory.embedding_provider = Some(embedder);
    }

    /// Installs an already-loaded character card and clears conversation history.
    ///
    /// Preferred by [`ene_runtime::EneHandle::open`]: config file I/O stays in
    /// the host / `ene-config`; the session only receives the card value.
    pub fn set_card(&mut self, card: CharacterCardV3) -> Vec<ResolvedExpression> {
        self.character_card = Some(card.clone());
        self.current_card_path.clear();
        self.history.conversation_history.clear();
        self.memory.ccv3_memory_hash = None;
        resolve_expressions(&card)
    }

    /// Loads a character card from `path`, merges `character_settings.json` expressions,
    /// and clears the conversation history.
    pub fn load_card(
        &mut self,
        path: &str,
    ) -> Result<Vec<ResolvedExpression>, super::error::EneSessionError> {
        if self.current_card_path == path
            && self.character_card.is_some()
            && let Some(card) = &self.character_card
        {
            return Ok(resolve_expressions(card));
        }

        let file_content =
            std::fs::read_to_string(path).map_err(ene_config::EneConfigError::CardReadError)?;

        let card = serde_json::from_str::<CharacterCardV3>(&file_content)
            .map_err(ene_config::EneConfigError::JsonError)?;

        self.character_card = Some(card.clone());
        self.current_card_path = path.to_string();
        self.history.conversation_history.clear();
        self.memory.ccv3_memory_hash = None;

        Ok(resolve_expressions(&card))
    }

    /// Appends a user message and trims history if it exceeds `max_history_turns * 2`.
    ///
    /// Trimming is suspended while a session split is pending (see
    /// [`Self::mark_split_pending`]) so the snapshot boundary recorded for the
    /// split remains valid until the split is applied.
    pub fn add_user_message(&mut self, input: &str) {
        self.history.conversation_history.push(HistoryEntry {
            role: Role::User,
            content: input.to_string(),
        });
        if self.state.pending_split_snapshot_len.is_none() {
            self.history.trim_history();
        }
    }

    /// Appends an assistant message and trims history if it exceeds `max_history_turns * 2`.
    ///
    /// Trimming is suspended while a session split is pending — see
    /// [`Self::add_user_message`].
    pub fn add_assistant_message(&mut self, text: &str) {
        self.history.conversation_history.push(HistoryEntry {
            role: Role::Assistant,
            content: text.to_string(),
        });
        if self.state.pending_split_snapshot_len.is_none() {
            self.history.trim_history();
        }
    }

    /// Records the current history length as the snapshot boundary for a
    /// pending session split. The boundary is used by
    /// [`Self::apply_split_result`] to discard only the entries that were in
    /// the split snapshot, keeping the triggering turn and any messages
    /// added while the split was in flight.
    pub fn mark_split_pending(&mut self) {
        self.state.pending_split_snapshot_len = Some(self.history.conversation_history.len());
    }

    /// Returns `true` if a session split is currently in flight.
    #[must_use]
    pub fn is_split_pending(&self) -> bool {
        self.state.pending_split_snapshot_len.is_some()
    }

    /// Clears the split-pending marker without applying a result. Used when
    /// the split task reports a non-fatal error (e.g. `SplitNotNeeded`) so
    /// history trimming can resume.
    pub fn clear_split_pending(&mut self) {
        self.state.pending_split_snapshot_len = None;
    }

    /// Applies a completed split result to the session.
    ///
    /// The conversation history is truncated to drop everything *before* the
    /// last entry of the snapshot that was taken in [`Self::mark_split_pending`]
    /// (and recorded in `split.snapshot_len`). The last snapshot entry — the
    /// triggering user turn — and any messages appended to the history while
    /// the split was running are preserved in the live history. The session
    /// ID is rotated to the split's `new_session_id` and the split-pending
    /// marker is cleared.
    pub fn apply_split_result(&mut self, split: &SplitResult) {
        let effective = split
            .snapshot_len
            .min(self.history.conversation_history.len());
        if effective > 0 {
            self.history.conversation_history.drain(0..(effective - 1));
        }
        self.memory.session_id = split.new_session_id.clone();
        self.memory.session_started_at = Utc::now();
        self.state.pending_split_snapshot_len = None;
    }

    /// Processes a streaming text chunk, splitting it into text deltas and special tokens
    /// (e.g., `<|emo:happy|>`). Appends text to the display buffer.
    ///
    /// Returns `(text_deltas, special_tokens)`.
    pub fn process_delta(&mut self, chunk: &str) -> (Vec<String>, Vec<String>) {
        let (text_deltas, special_tokens) =
            split_text_and_special_tokens(&mut self.display.token_carry, chunk);
        for delta in &text_deltas {
            self.display.display_buffer.push_str(delta);
        }
        (text_deltas, special_tokens)
    }

    /// Finalizes the current response: flushes any remaining token carry, commits the
    /// display buffer as an assistant message, and returns any lingering token fragment.
    pub fn finalize_response(&mut self) -> Option<String> {
        let mut tail = None;
        if !self.display.token_carry.is_empty() {
            let t = std::mem::take(&mut self.display.token_carry);
            self.display.display_buffer.push_str(&t);
            tail = Some(t);
        }

        let assistant_text = self.display.display_buffer.clone();
        self.add_assistant_message(&assistant_text);

        tail
    }

    /// Resets the display buffer (used when a response is interrupted or discarded).
    pub fn reset_display_buffer(&mut self) {
        self.display.display_buffer.clear();
        self.display.token_carry.clear();
    }

    /// Resets all session state (history, display, turn count) and returns a new session ID.
    pub fn reset_session(&mut self) -> SessionId {
        let new_id = generate_session_id();
        self.history.conversation_history.clear();
        self.display.display_buffer.clear();
        self.display.token_carry.clear();
        self.memory.session_id = new_id.clone();
        self.memory.session_started_at = chrono::Utc::now();
        self.memory.pending_embedding = None;
        self.state.last_input_embedding = None;
        self.state.last_message_time = None;
        self.state.current_turn_count = 0;
        self.state.last_resolved_expression.clear();
        self.state.last_expression_changed_at = None;
        new_id
    }

    /// Stores an embedding for the current pending user input (used for memory search).
    pub fn set_pending_embedding(&mut self, embedding: Vec<f32>) {
        self.memory.pending_embedding = Some(embedding);
    }

    /// Stores the embedding of the most recent user input (used for topic boundary detection).
    pub fn set_last_input_embedding(&mut self, embedding: Vec<f32>) {
        self.state.last_input_embedding = Some(embedding);
    }

    /// Tracks timing and turn count after a user sends a message.
    pub fn record_user_input(&mut self) {
        self.state.current_turn_count += 1;
        self.state.last_message_time = Some(Utc::now());
    }

    /// Tracks timing and turn count after the assistant sends a response.
    pub fn record_assistant_response(&mut self) {
        self.state.current_turn_count += 1;
        self.state.last_message_time = Some(Utc::now());
    }

    /// Elapsed time since the last expression change (for arbiter hysteresis).
    #[must_use]
    pub fn expression_elapsed(&self) -> Option<std::time::Duration> {
        self.state.last_expression_changed_at.map(|ts| {
            Utc::now()
                .signed_duration_since(ts)
                .to_std()
                .unwrap_or(std::time::Duration::ZERO)
        })
    }

    /// Records a resolved expression for in-session hysteresis tracking.
    pub fn record_expression_change(&mut self, name: &str) {
        if self.state.last_resolved_expression != name {
            self.state.last_resolved_expression = name.to_string();
            self.state.last_expression_changed_at = Some(Utc::now());
        }
    }

    /// Returns the last resolved expression name for this session.
    #[must_use]
    pub fn last_resolved_expression(&self) -> &str {
        &self.state.last_resolved_expression
    }

    /// Previous expression and elapsed time for arbiter hysteresis.
    ///
    /// Falls back to persisted [`ene_store::AffectState::last_expression`] and
    /// `updated_at` when the in-session tracker is empty (e.g. after restart).
    #[must_use]
    pub fn expression_context<'a>(
        &'a self,
        affect: &'a ene_store::AffectState,
    ) -> (Cow<'a, str>, Option<std::time::Duration>) {
        if !self.state.last_resolved_expression.is_empty() {
            return (
                Cow::Borrowed(self.last_resolved_expression()),
                self.expression_elapsed(),
            );
        }
        if affect.last_expression.is_empty() {
            return (Cow::Borrowed(""), None);
        }
        let elapsed = affect.updated_at.map(|ts| {
            Utc::now()
                .signed_duration_since(ts)
                .to_std()
                .unwrap_or(std::time::Duration::ZERO)
        });
        (Cow::Borrowed(&affect.last_expression), elapsed)
    }

    /// Returns the current character name, or `"default"` if no card is loaded.
    #[must_use]
    pub fn card_name(&self) -> &str {
        self.character_card
            .as_ref()
            .map_or("default", |c| c.data.get_character_name())
    }

    /// Returns a reference to the conversation history entries.
    #[must_use]
    pub fn history(&self) -> &[HistoryEntry] {
        &self.history.conversation_history
    }

    /// Returns the current session ID.
    #[must_use]
    pub fn session_id(&self) -> &SessionId {
        &self.memory.session_id
    }

    /// Returns when the session started (UTC).
    #[must_use]
    pub fn session_started_at(&self) -> DateTime<Utc> {
        self.memory.session_started_at
    }

    /// Returns the current conversation turn count.
    #[must_use]
    pub fn current_turn_count(&self) -> usize {
        self.state.current_turn_count
    }

    /// Trim in-memory history, keeping only the last `keep` messages.
    pub fn trim_history_keep_last(&mut self, keep: usize) {
        let len = self.history.conversation_history.len();
        if len > keep {
            self.history.conversation_history.drain(0..(len - keep));
        }
    }

    /// Returns the timestamp of the last received message, if any.
    #[must_use]
    pub fn last_message_time(&self) -> Option<DateTime<Utc>> {
        self.state.last_message_time
    }

    /// Returns the number of minutes elapsed since the session started.
    #[must_use]
    pub fn session_elapsed_minutes(&self) -> i64 {
        (Utc::now() - self.memory.session_started_at).num_minutes()
    }

    /// Polls for a completed split result and applies it to the session.
    ///
    /// If a split has completed, the conversation history is cleared and the
    /// session ID is updated to the new one from the split result.
    pub fn apply_pending_split(
        &mut self,
        pending_split: &mut Option<PendingSplitTask>,
    ) -> Option<Result<SplitResult, EneSessionError>> {
        let result = poll_split_result(pending_split)?;
        if let Ok(ref split_result) = result {
            self.reset_session();
            self.memory.session_id = split_result.new_session_id.clone();
        }
        Some(result)
    }

    /// Builds a [`SplitTaskInput`] from the current session state and settings.
    ///
    /// Returns `None` if the memory store or embedding provider has not been initialized.
    pub fn prepare_split_input(
        &self,
        config: &EneConfig,
        user_input: &str,
        user_name: &str,
        provider: Arc<dyn ene_ai::LlmProvider>,
    ) -> Option<SplitTaskInput> {
        let store = self.memory.memory_store.clone()?;
        let embedder = self.memory.embedding_provider.clone()?;
        let session_config = config
            .get_section::<super::SessionConfig>()
            .unwrap_or_default();

        Some(SplitTaskInput {
            last_input_embedding: self.state.last_input_embedding.clone(),
            last_message_time: self.state.last_message_time,
            current_turn_count: self.state.current_turn_count,
            history_len: self.history.conversation_history.len(),
            user_input: user_input.to_string(),
            session_config,
            provider,
            history: self.history.conversation_history.clone(),
            session_id: self.memory.session_id.clone(),
            card_name: CardName::from(self.card_name()),
            user_name: user_name.to_string(),
            store,
            embedder,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn split_with_len(new_id: &str, snapshot_len: usize) -> SplitResult {
        SplitResult {
            reason: crate::SplitReason::Manual,
            summary: String::new(),
            key_facts: Vec::new(),
            new_session_id: SessionId::from(new_id.to_string()),
            snapshot_len,
        }
    }

    #[test]
    fn apply_split_keeps_triggering_and_subsequent_turns() {
        let mut s = ConversationSession::default();

        for t in ["turn 1", "turn 2", "turn 3"] {
            s.add_user_message(t);
        }
        s.mark_split_pending();

        // Two more user turns arrive while the split is in flight.
        s.add_user_message("turn 4");
        s.add_user_message("turn 5");

        // Sanity: trim is suspended while a split is pending, so the snapshot
        // boundary is preserved.
        assert_eq!(s.history().len(), 5);

        s.apply_split_result(&split_with_len("session_new", 3));

        // Triggering turn 3 plus turns 4 and 5 are retained; turns 1 and 2
        // are dropped.
        let kept: Vec<&str> = s.history().iter().map(|e| e.content.as_str()).collect();
        assert_eq!(kept, vec!["turn 3", "turn 4", "turn 5"]);
        assert_eq!(s.session_id().as_str(), "session_new");
        assert!(!s.is_split_pending());
    }

    #[test]
    fn apply_split_with_no_extra_turns_keeps_triggering_turn() {
        let mut s = ConversationSession::default();
        s.add_user_message("turn 1");
        s.add_user_message("turn 2");
        s.mark_split_pending();

        s.apply_split_result(&split_with_len("session_new", 2));

        let kept: Vec<&str> = s.history().iter().map(|e| e.content.as_str()).collect();
        assert_eq!(kept, vec!["turn 2"]);
    }

    #[test]
    fn clear_split_pending_resumes_trimming() {
        let mut s = ConversationSession::default();
        s.history.max_history_turns = 1;
        s.add_user_message("turn 1");
        s.add_user_message("turn 2");
        s.mark_split_pending();
        s.add_user_message("turn 3");
        s.add_user_message("turn 4");
        // No trim while pending; all 4 entries present.
        assert_eq!(s.history().len(), 4);
        s.clear_split_pending();
        s.add_user_message("turn 5");
        // Trim resumed: cap is max_history_turns * 2 = 2 entries.
        assert_eq!(s.history().len(), 2);
    }
}
