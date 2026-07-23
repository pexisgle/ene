use super::session_split::generate_session_id;
use super::special_token::split_text_and_special_tokens;
use super::types::SessionId;
use chrono::{DateTime, Utc};
use ene_ai::EmbeddingProvider;
use ene_ai::Role;
use ene_config::{CharacterCardV3, ResolvedExpression, resolve_expressions};

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
    /// Cached hash of the last synced `CCv3` character memory index.
    pub ccv3_memory_hash: Option<u64>,
}

/// Snapshot of a turn that was interrupted mid-response (barge-in / cancel).
///
/// Recorded when the user cancels an in-flight turn so the next turn's prompt
/// can acknowledge the interruption and memory candidates can be tagged.
/// Turn identifiers are plain strings to keep `ene-mind` free of any
/// `ene-runtime` dependency (architecture boundary).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InterruptedState {
    /// Identifier of the interrupted turn.
    pub turn_id: String,
    /// Character range of the response that had been spoken before interruption.
    pub spoken_char_range: std::ops::Range<usize>,
    /// The partial assistant text that had been produced so far.
    pub partial_text: String,
    /// When the interruption was recorded.
    pub interrupted_at: DateTime<Utc>,
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
    /// Snapshot of the most recently interrupted turn, if any (#206).
    interrupted: Option<InterruptedState>,
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
            .finish_non_exhaustive()
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
                max_history_turns: 20,
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
                last_resolved_expression: String::new(),
                last_expression_changed_at: None,
            },
            character_card: None,
            current_card_path: String::new(),
            interrupted: None,
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
    pub fn set_card(&mut self, card: &CharacterCardV3) -> Vec<ResolvedExpression> {
        self.character_card = Some(card.clone());
        self.current_card_path.clear();
        self.history.conversation_history.clear();
        self.memory.ccv3_memory_hash = None;
        resolve_expressions(card)
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
    pub fn add_user_message(&mut self, input: &str) {
        self.history.conversation_history.push(HistoryEntry {
            role: Role::User,
            content: input.to_string(),
        });
        self.history.trim_history();
    }

    /// Appends an assistant message and trims history if it exceeds `max_history_turns * 2`.
    pub fn add_assistant_message(&mut self, text: &str) {
        self.history.conversation_history.push(HistoryEntry {
            role: Role::Assistant,
            content: text.to_string(),
        });
        self.history.trim_history();
    }

    /// Processes a streaming text chunk, splitting it into text deltas and special tokens
    /// (e.g., `<|perf:expr=happy|>`). Appends text to the display buffer.
    ///
    /// Returns `(text_deltas, special_tokens)`.
    pub fn process_delta(&mut self, chunk: &str) -> (Vec<String>, Vec<String>) {
        let (text_deltas, special_tokens) =
            split_text_and_special_tokens(&mut self.display.token_carry, chunk);
        for delta in &text_deltas {
            self.display.display_buffer.push_str(delta);
        }
        // Guard against unbounded growth from unclosed special token markers.
        const MAX_TOKEN_CARRY: usize = 4096;
        if self.display.token_carry.len() > MAX_TOKEN_CARRY {
            self.display
                .display_buffer
                .push_str(&self.display.token_carry);
            self.display.token_carry.clear();
        }
        (text_deltas, special_tokens)
    }

    /// Finalizes the current response: flushes any remaining token carry, commits the
    /// display buffer as an assistant message, and returns any lingering token fragment.
    pub fn finalize_response(&mut self) -> Option<String> {
        let tail = if self.display.token_carry.is_empty() {
            None
        } else {
            let t = std::mem::take(&mut self.display.token_carry);
            self.display.display_buffer.push_str(&t);
            Some(t)
        };

        let assistant_text = std::mem::take(&mut self.display.display_buffer);
        self.add_assistant_message(&assistant_text);

        tail
    }

    /// Resets the display buffer (used when a response is interrupted or discarded).
    pub fn reset_display_buffer(&mut self) {
        self.display.display_buffer.clear();
        self.display.token_carry.clear();
    }

    /// Records that the given turn was interrupted mid-response (#206).
    ///
    /// `spoken_text` is the portion of the assistant response that had been
    /// produced (and typically spoken via TTS) before the interruption, and
    /// `spoken_chars` is its length in characters. The partial text is also
    /// committed to history so the exchange is not lost.
    pub fn mark_interrupted(&mut self, turn_id: &str, spoken_text: &str, spoken_chars: usize) {
        let clamped_chars = spoken_chars.min(spoken_text.chars().count());
        if !spoken_text.is_empty() {
            self.add_assistant_message(spoken_text);
        }
        self.interrupted = Some(InterruptedState {
            turn_id: turn_id.to_string(),
            spoken_char_range: 0..clamped_chars,
            partial_text: spoken_text.to_string(),
            interrupted_at: Utc::now(),
        });
        self.reset_display_buffer();
    }

    /// Consumes and clears the pending interruption snapshot, if any (#206).
    ///
    /// Called when composing the next turn's prompt so the model can
    /// acknowledge or resume the interrupted response exactly once.
    pub fn take_interruption(&mut self) -> Option<InterruptedState> {
        self.interrupted.take()
    }

    /// Whether an interruption snapshot is currently pending (#206).
    pub const fn has_pending_interruption(&self) -> bool {
        self.interrupted.is_some()
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
        self.interrupted = None;
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

    /// Last expression name resolved during this session.
    pub fn last_resolved_expression(&self) -> &str {
        &self.state.last_resolved_expression
    }

    /// Previous expression and elapsed time for arbiter hysteresis.
    ///
    /// Falls back to persisted [`ene_store::AffectState::last_expression`] and
    /// `updated_at` when the in-session tracker is empty (e.g. after restart).
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
    pub fn card_name(&self) -> &str {
        self.character_card
            .as_ref()
            .map_or("default", |c| c.data.get_character_name())
    }

    /// Returns a reference to the conversation history entries.
    pub fn history(&self) -> &[HistoryEntry] {
        &self.history.conversation_history
    }

    /// Unique identifier for this session.
    pub const fn session_id(&self) -> &SessionId {
        &self.memory.session_id
    }

    /// Timestamp when this session was created.
    pub const fn session_started_at(&self) -> DateTime<Utc> {
        self.memory.session_started_at
    }

    /// Number of turns completed in this session.
    pub const fn current_turn_count(&self) -> usize {
        self.state.current_turn_count
    }

    /// Trim in-memory history, keeping only the last `keep` messages.
    pub fn trim_history_keep_last(&mut self, keep: usize) {
        let len = self.history.conversation_history.len();
        if len > keep {
            self.history.conversation_history.drain(0..(len - keep));
        }
    }

    /// Timestamp of the most recent user message.
    pub const fn last_message_time(&self) -> Option<DateTime<Utc>> {
        self.state.last_message_time
    }

    /// Elapsed minutes since session start.
    pub fn session_elapsed_minutes(&self) -> i64 {
        (Utc::now() - self.memory.session_started_at).num_minutes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_trims_after_max_turns() {
        let mut s = ConversationSession::default();
        s.history.max_history_turns = 1;
        s.add_user_message("turn 1");
        s.add_user_message("turn 2");
        s.add_user_message("turn 3");
        assert_eq!(s.history().len(), 2);
    }

    #[test]
    fn mark_interrupted_records_and_take_clears() {
        let mut s = ConversationSession::default();
        s.mark_interrupted("turn-1", "hello wor", 5);

        let state = s.take_interruption().expect("interruption recorded");
        assert_eq!(state.turn_id, "turn-1");
        assert_eq!(state.partial_text, "hello wor");
        assert_eq!(state.spoken_char_range, 0..5);
        // Partial text committed to history.
        assert_eq!(
            s.history().last().map(|e| e.content.as_str()),
            Some("hello wor")
        );
        // Consumed exactly once.
        assert!(s.take_interruption().is_none());
    }

    #[test]
    fn reset_session_clears_interruption() {
        let mut s = ConversationSession::default();
        s.mark_interrupted("turn-1", "partial", 3);
        s.reset_session();
        assert!(s.take_interruption().is_none());
    }
}
