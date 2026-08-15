#![expect(
    clippy::arithmetic_side_effects,
    reason = "mind pipeline uses intentional turn/score/index arithmetic"
)]
use super::session_split::generate_session_id;
use super::special_token::{StreamPiece, split_text_and_special_tokens_ordered};
use super::types::SessionId;
use chrono::{DateTime, Utc};
use ene_ai::EmbeddingProvider;
use ene_ai::Role;
use ene_card::{CharacterCardV3, ResolvedExpression, resolve_expressions};

use crate::lifecycle::HistoryEntry;
use ene_core::MemoryPort;
use std::borrow::Cow;
use std::sync::Arc;

#[derive(Clone, Debug)]
pub struct ConversationHistory {
    pub conversation_history: Vec<HistoryEntry>,
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

#[derive(Clone, Debug, Default)]
pub struct DisplayState {
    pub display_buffer: String,
    pub token_carry: String,
}

/// Upper bound for the incomplete-marker carry buffer.
///
/// `token_carry` holds text from the moment a `<|` opener is seen until the
/// matching `|>` closer arrives. If a model emits `<|` and never closes it,
/// everything after the opener would be withheld from both the display and TTS
/// until this bound is reached, so a large cap makes the avatar appear frozen
/// for several sentences.
///
/// The longest well-formed marker is a motion cue carrying a name from the
/// card's motion catalog plus a layer suffix — comfortably under 128 chars
/// (see `special_token.rs`). 256 leaves generous headroom while keeping a
/// stalled stream from withholding more than a sentence of output.
const MAX_TOKEN_CARRY: usize = 256;

#[derive(Clone)]
pub struct MemoryContext {
    pub memory_store: Option<Arc<dyn MemoryPort>>,
    pub embedding_provider: Option<Arc<dyn EmbeddingProvider>>,
    pub session_id: SessionId,
    pub session_started_at: chrono::DateTime<chrono::Utc>,
    pub pending_embedding: Option<Vec<f32>>,
    /// Cached hash of the last synced `CCv3` character memory index.
    pub ccv3_memory_hash: Option<u64>,
    /// L1 recall cache shared across turns and runtime mutation handles.
    pub recall_cache: Option<Arc<crate::recall::MemoryRecallCache>>,
}

/// Snapshot of a turn that was interrupted mid-response (barge-in / cancel).
///
/// Recorded when the user cancels an in-flight turn so the next turn's prompt
/// can acknowledge the interruption and memory candidates can be tagged.
/// Turn identifiers are plain strings to keep `ene-mind` free of any
/// `ene-runtime` dependency (architecture boundary).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InterruptedState {
    pub turn_id: String,
    /// Character range of the response that had been spoken before interruption.
    pub spoken_char_range: std::ops::Range<usize>,
    pub partial_text: String,
    pub interrupted_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Default)]
pub struct SessionState {
    pub last_input_embedding: Option<Vec<f32>>,
    pub last_message_time: Option<DateTime<Utc>>,
    pub current_turn_count: usize,
    pub last_resolved_expression: String,
    pub last_expression_changed_at: Option<DateTime<Utc>>,
    pub topic_boundary: super::topic_boundary::TopicBoundaryTracker,
}

/// Central session container holding conversation history, display state, memory context,
/// and the loaded character card. Shared between the streaming engine and the CLI/GUI frontends.
#[derive(Clone)]
pub struct ConversationSession {
    pub(crate) history: ConversationHistory,
    pub display: DisplayState,
    pub memory: MemoryContext,
    pub(crate) state: SessionState,
    pub character_card: Option<CharacterCardV3>,
    current_card_path: String,
    /// Index of the greeting chosen for this session (`0` = `first_mes`,
    /// `i+1` = `alternate_greetings[i]`); `None` before a greeting is applied.
    /// Drives the `@@is_greeting` lorebook decorator (`CCv3` `SPEC_V3.md`).
    active_greeting_index: Option<u32>,
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
                recall_cache: Some(Arc::new(crate::recall::MemoryRecallCache::new())),
            },
            state: SessionState {
                last_input_embedding: None,
                last_message_time: None,
                current_turn_count: 0,
                last_resolved_expression: String::new(),
                last_expression_changed_at: None,
                topic_boundary: super::topic_boundary::TopicBoundaryTracker::new(),
            },
            character_card: None,
            current_card_path: String::new(),
            active_greeting_index: None,
            interrupted: None,
        }
    }

    pub fn init_memory(
        &mut self,
        store: Arc<dyn MemoryPort>,
        embedder: Arc<dyn EmbeddingProvider>,
    ) {
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
        self.active_greeting_index = None;
        self.memory.ccv3_memory_hash = None;
        resolve_expressions(card)
    }

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
        self.active_greeting_index = None;
        self.memory.ccv3_memory_hash = None;

        Ok(resolve_expressions(&card))
    }

    pub fn add_user_message(&mut self, input: &str) {
        self.history.conversation_history.push(HistoryEntry {
            role: Role::User,
            content: input.to_string(),
        });
        self.history.trim_history();
    }

    pub fn add_assistant_message(&mut self, text: &str) {
        self.history.conversation_history.push(HistoryEntry {
            role: Role::Assistant,
            content: text.to_string(),
        });
        self.history.trim_history();
    }

    /// Opens the session with `text` as the character's greeting message and
    /// records `index` as the active greeting.
    ///
    /// The greeting is an ordinary assistant history entry (so it counts
    /// toward `@@activate_only_after` like any character message) but does not
    /// bump the turn count: it is not a model turn. Callers are responsible
    /// for validating that the session has no history yet.
    pub fn apply_greeting(&mut self, text: &str, index: u32) {
        self.add_assistant_message(text);
        self.active_greeting_index = Some(index);
    }

    #[must_use]
    pub const fn active_greeting_index(&self) -> Option<u32> {
        self.active_greeting_index
    }

    pub fn process_delta(&mut self, chunk: &str) -> (Vec<String>, Vec<String>) {
        let mut text_deltas = Vec::new();
        let mut special_tokens = Vec::new();
        for piece in self.process_delta_ordered(chunk) {
            match piece {
                StreamPiece::Text(text) => text_deltas.push(text),
                StreamPiece::Marker(token) => special_tokens.push(token),
            }
        }
        (text_deltas, special_tokens)
    }

    /// Processes a streaming text chunk, returning an ordered stream of text
    /// deltas and special-token markers so callers can map marker positions
    /// onto the clean text.
    ///
    /// Appends text to the display buffer. See
    /// [`split_text_and_special_tokens_ordered`] for the ordering contract.
    pub fn process_delta_ordered(&mut self, chunk: &str) -> Vec<StreamPiece> {
        let mut pieces =
            split_text_and_special_tokens_ordered(&mut self.display.token_carry, chunk);
        // Guard against unbounded growth from an unterminated marker: if the
        // carry exceeds `MAX_TOKEN_CARRY`, abandon the marker and release the
        // withheld text to the live output stream so it reaches the display and
        // TTS instead of being held back (or silently dropped).
        if self.display.token_carry.len() > MAX_TOKEN_CARRY {
            let abandoned = std::mem::take(&mut self.display.token_carry);
            pieces.push(StreamPiece::Text(abandoned));
        }
        for piece in &pieces {
            if let StreamPiece::Text(text) = piece {
                self.display.display_buffer.push_str(text);
            }
        }
        pieces
    }

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

    pub fn reset_display_buffer(&mut self) {
        self.display.display_buffer.clear();
        self.display.token_carry.clear();
    }

    /// Records that the given turn was interrupted mid-response.
    ///
    /// `spoken_text` is the portion of the assistant response that had been
    /// produced (and typically spoken via TTS) before the interruption, and
    /// `spoken_chars` is its length in characters. The partial text is also
    /// committed to history so the exchange is not lost.
    ///
    /// The assistant turn count is bumped so user/assistant turn accounting
    /// stays symmetric with the normal completion path, which calls
    /// [`record_assistant_response`](Self::record_assistant_response) after
    /// committing the full response.
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
        self.record_assistant_response();
    }

    /// Consumes and clears the pending interruption snapshot, if any.
    ///
    /// Called when composing the next turn's prompt so the model can
    /// acknowledge or resume the interrupted response exactly once.
    pub fn take_interruption(&mut self) -> Option<InterruptedState> {
        self.interrupted.take()
    }

    pub const fn has_pending_interruption(&self) -> bool {
        self.interrupted.is_some()
    }

    pub fn reset_session(&mut self) -> SessionId {
        let new_id = generate_session_id();
        // The cache is shared with runtime mutation handles, so it is cleared
        // in place rather than replaced; the fresh session id also makes old
        // gather keys unreachable.
        if let Some(cache) = &self.memory.recall_cache {
            cache.invalidate_all();
        }
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
        self.state.topic_boundary.reset_topic();
        self.active_greeting_index = None;
        self.interrupted = None;
        new_id
    }

    pub fn set_pending_embedding(&mut self, embedding: Vec<f32>) {
        self.memory.pending_embedding = Some(embedding);
    }

    pub fn set_last_input_embedding(&mut self, embedding: Vec<f32>) {
        self.state.last_input_embedding = Some(embedding);
    }

    /// Runs topic-boundary detection for the just-completed turn.
    ///
    /// Consumes the stored [`last_input_embedding`](Self::set_last_input_embedding)
    /// as the turn's embedding, scores it against the running topic centroid,
    /// and updates the detector state. `utterance_chars` is the length of the
    /// user utterance in characters (short backchannels are ignored). Returns
    /// `None` when detection is disabled or no embedding is available for the
    /// turn. Compression consumes the returned score; session split does not.
    pub fn detect_topic_boundary(
        &mut self,
        config: &crate::config::TopicBoundaryConfig,
        utterance_chars: usize,
    ) -> Option<super::topic_boundary::TopicBoundarySignal> {
        let embedding = self.state.last_input_embedding.clone()?;
        Some(self.state.topic_boundary.observe_turn(
            config,
            &embedding,
            utterance_chars,
            Utc::now(),
        ))
    }

    pub fn record_user_input(&mut self) {
        self.state.current_turn_count += 1;
        self.state.last_message_time = Some(Utc::now());
    }

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

    pub fn record_expression_change(&mut self, name: &str) {
        if self.state.last_resolved_expression != name {
            self.state.last_resolved_expression = name.to_string();
            self.state.last_expression_changed_at = Some(Utc::now());
        }
    }

    pub fn last_resolved_expression(&self) -> &str {
        &self.state.last_resolved_expression
    }

    /// Previous expression and elapsed time for arbiter hysteresis.
    ///
    /// Falls back to persisted [`ene_core::AffectState::last_expression`] and
    /// `updated_at` when the in-session tracker is empty (e.g. after restart).
    pub fn expression_context<'a>(
        &'a self,
        affect: &'a ene_core::AffectState,
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

    /// Returns the stable current character identity, or `"default"` if no card is loaded.
    pub fn card_name(&self) -> &str {
        self.character_card
            .as_ref()
            .map_or("default", |c| c.data.get_character_id())
    }

    pub fn history(&self) -> &[HistoryEntry] {
        &self.history.conversation_history
    }

    pub const fn session_id(&self) -> &SessionId {
        &self.memory.session_id
    }

    pub const fn session_started_at(&self) -> DateTime<Utc> {
        self.memory.session_started_at
    }

    pub const fn current_turn_count(&self) -> usize {
        self.state.current_turn_count
    }

    pub fn trim_history_keep_last(&mut self, keep: usize) {
        let len = self.history.conversation_history.len();
        if len > keep {
            self.history.conversation_history.drain(0..(len - keep));
        }
    }

    pub const fn last_message_time(&self) -> Option<DateTime<Utc>> {
        self.state.last_message_time
    }

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
        assert_eq!(
            s.history().last().map(|e| e.content.as_str()),
            Some("hello wor")
        );
        assert!(s.take_interruption().is_none());
    }

    #[test]
    fn reset_session_clears_interruption() {
        let mut s = ConversationSession::default();
        s.mark_interrupted("turn-1", "partial", 3);
        s.reset_session();
        assert!(s.take_interruption().is_none());
    }

    #[test]
    fn apply_greeting_records_message_and_index() {
        let mut s = ConversationSession::default();
        s.apply_greeting("Hi there!", 2);
        assert_eq!(s.active_greeting_index(), Some(2));
        let history = s.history();
        assert_eq!(history.len(), 1);
        let entry = history.first().expect("greeting recorded");
        assert_eq!(entry.role, Role::Assistant);
        assert_eq!(entry.content, "Hi there!");
        // A greeting is not a model turn.
        assert_eq!(s.current_turn_count(), 0);
    }

    #[test]
    fn reset_session_clears_active_greeting() {
        let mut s = ConversationSession::default();
        s.apply_greeting("Hello", 0);
        s.reset_session();
        assert_eq!(s.active_greeting_index(), None);
        assert!(s.history().is_empty());
    }

    #[test]
    fn process_delta_emits_complete_marker_and_text() {
        let mut s = ConversationSession::default();
        let (text, tokens) = s.process_delta("Hi <|perf:expr=happy|> there");
        assert_eq!(text, vec!["Hi ", " there"]);
        assert_eq!(tokens, vec!["<|perf:expr=happy|>"]);
        assert!(s.display.token_carry.is_empty());
        assert_eq!(s.display.display_buffer, "Hi  there");
    }

    #[test]
    fn process_delta_ordered_preserves_text_marker_interleaving() {
        let mut s = ConversationSession::default();
        let pieces = s.process_delta_ordered("<|perf:expr=happy|> A <|perf:expr=sad|> B");
        assert_eq!(
            pieces,
            vec![
                StreamPiece::Marker("<|perf:expr=happy|>".to_string()),
                StreamPiece::Text(" A ".to_string()),
                StreamPiece::Marker("<|perf:expr=sad|>".to_string()),
                StreamPiece::Text(" B".to_string()),
            ]
        );
        assert_eq!(s.display.display_buffer, " A  B");
    }

    #[test]
    fn process_delta_buffers_short_unterminated_marker() {
        let mut s = ConversationSession::default();
        let (text, tokens) = s.process_delta("Hello <|perf");
        assert_eq!(text, vec!["Hello "]);
        assert!(tokens.is_empty());
        // A short incomplete marker is still carried, awaiting its closer.
        assert_eq!(s.display.token_carry, "<|perf");
    }

    #[test]
    fn process_delta_releases_unterminated_marker_beyond_cap() {
        let mut s = ConversationSession::default();
        // An opener that is never closed, followed by well over the carry cap
        // of plain text. The withheld text must be released to the output
        // stream rather than buffered without bound.
        let filler = "a".repeat(MAX_TOKEN_CARRY + 1);
        let chunk = format!("<|{filler}");
        let (text, tokens) = s.process_delta(&chunk);

        assert!(tokens.is_empty());
        assert!(s.display.token_carry.is_empty());
        // The abandoned text reached the live output (display + TTS path).
        assert_eq!(text.concat(), chunk);
        assert_eq!(s.display.display_buffer, chunk);
    }

    #[test]
    fn process_delta_unterminated_marker_withholds_at_most_cap() {
        let mut s = ConversationSession::default();
        // Open a marker that is never closed, then keep feeding text. The
        // carry must never exceed the cap by more than a single chunk.
        s.process_delta("<|");
        let mut withheld_max = s.display.token_carry.len();
        for _ in 0..(MAX_TOKEN_CARRY * 4) {
            s.process_delta("x");
            withheld_max = withheld_max.max(s.display.token_carry.len());
        }
        assert!(
            withheld_max <= MAX_TOKEN_CARRY + 1,
            "carry grew to {withheld_max}, exceeding the cap"
        );
    }
}
