# `ConversationSession` & Session State Specifications

This document defines Ene's session state manager, including conversation history storage, in-memory string buffering, character card loading, and inline `<|perf:…|>` cue parsing.

---

## 1. Struct Definition & Main Session Methods

### `ConversationSession` (Public / Struct)
The central in-memory state keeper representing the active conversational thread:
*   `character_card: Option<CharacterCardV3>`: Parses character details and lorebooks.
*   `history: Vec<HistoryEntry>`: Chronological exchange of messages between user and assistant.
*   `display_buffer: String`: Plain conversational text stripped of special tokens, queued for the UI.
*   `session_id: SessionId`: Unique session UUID.

#### `new`
*   **Signature**: `pub fn new() -> Self`
*   **Description**: Constructs a new `ConversationSession` with empty history, display buffers, and generates a new session UUID.

#### `init_memory`
*   **Signature**: `pub fn init_memory(&mut self, store: Arc<MemoryStore>, embedder: Arc<dyn EmbeddingProvider>)`
*   **Description**: Links the SQLite memory store and vector embedding provider to the session context.

#### `set_card`
*   **Signature**: `pub fn set_card(&mut self, card: &CharacterCardV3) -> Vec<ResolvedExpression>`
*   **Description**: Binds character metadata to the session, updates the character card config, and extracts and returns resolved expressions.

#### `load_card`
*   **Signature**: `pub fn load_card(&mut self, path: &str) -> Result<Vec<ResolvedExpression>, super::error::EneSessionError>`
*   **Description**: Loads character cards from a file path and registers their expressions.

#### `add_user_message`
*   **Signature**: `pub fn add_user_message(&mut self, input: &str)`
*   **Description**: Appends a user message to the session's active history array.

#### `add_assistant_message`
*   **Signature**: `pub fn add_assistant_message(&mut self, text: &str)`
*   **Description**: Appends an assistant message to history.

#### `process_delta`
*   **Signature**: `pub fn process_delta(&mut self, chunk: &str) -> (Vec<String>, Vec<String>)`
*   **Description**: Processes streaming token deltas. Splits them into clean conversational text (appended to `display_buffer`) and performance cues.

#### `finalize_response`
*   **Signature**: `pub fn finalize_response(&mut self) -> Option<String>`
*   **Description**: Cleans up the active display buffer and returns the complete text of the assistant's response.

#### `reset_display_buffer`
*   **Signature**: `pub fn reset_display_buffer(&mut self)`
*   **Description**: Clears the session's temporary streaming text buffer.

#### `reset_session`
*   **Signature**: `pub fn reset_session(&mut self) -> SessionId`
*   **Description**: Clears history and buffers, and allocates a new session ID.

#### `set_pending_embedding`
*   **Signature**: `pub fn set_pending_embedding(&mut self, embedding: Vec<f32>)`
*   **Description**: Buffers computed vectors for the next write cycle.

#### `set_last_input_embedding`
*   **Signature**: `pub fn set_last_input_embedding(&mut self, embedding: Vec<f32>)`
*   **Description**: Records the vector embedding of the user's latest query.

#### `record_user_input`
*   **Signature**: `pub fn record_user_input(&mut self)`
*   **Description**: Records metrics when a user message is received.

#### `record_assistant_response`
*   **Signature**: `pub fn record_assistant_response(&mut self)`
*   **Description**: Records metrics when an assistant response is finalized.

#### `expression_elapsed`
*   **Signature**: `pub fn expression_elapsed(&self) -> Option<std::time::Duration>`
*   **Description**: Returns the time elapsed since the mascot's last visual expression change.

#### `record_expression_change`
*   **Signature**: `pub fn record_expression_change(&mut self, name: &str)`
*   **Description**: Records the timestamp and name of a visual expression transition.

#### `last_resolved_expression`
*   **Signature**: `pub fn last_resolved_expression(&self) -> &str`
*   **Description**: Returns the name of the currently active visual expression.

#### `expression_context`
*   **Signature**: `pub fn expression_context<'a>(&'a self, affect: &'a ene_store::AffectState) -> (Cow<'a, str>, Option<std::time::Duration>)`
*   **Description**: Computes active expression states and transition times.

#### `card_name`
*   **Signature**: `pub fn card_name(&self) -> &str`
*   **Description**: Returns the active character card name, or a default fallback.

#### `history`
*   **Signature**: `pub fn history(&self) -> &[HistoryEntry]`
*   **Description**: Accesses the active conversation history log.

#### `session_id`
*   **Signature**: `pub const fn session_id(&self) -> &SessionId`
*   **Description**: Returns the active session identifier.

#### `session_started_at`
*   **Signature**: `pub const fn session_started_at(&self) -> DateTime<Utc>`
*   **Description**: Returns the session creation timestamp.

#### `current_turn_count`
*   **Signature**: `pub const fn current_turn_count(&self) -> usize`
*   **Description**: Returns the total number of dialogue exchanges in the current session.

#### `trim_history_keep_last`
*   **Signature**: `pub fn trim_history_keep_last(&mut self, keep: usize)`
*   **Description**: Trims history, retaining only the most recent `keep` messages.

#### `last_message_time`
*   **Signature**: `pub const fn last_message_time(&self) -> Option<DateTime<Utc>>`
*   **Description**: Returns the timestamp of the last message in history.

#### `session_elapsed_minutes`
*   **Signature**: `pub fn session_elapsed_minutes(&self) -> i64`
*   **Description**: Calculates the elapsed time in minutes since the session started.

---

## 2. Session Utilities & ID Generation (`session_split.rs`)

#### `generate_session_id`
*   **Signature**: `pub fn generate_session_id() -> SessionId`
*   **Description**: Generates a new unique `SessionId` UUID.

---

## 3. Inline Special Tokens & Performance Cues (`special_token.rs`)

If the appraisal emotion engine is disabled, the LLM outputs inline tags to trigger mascot animations:

#### `split_text_and_special_tokens`
*   **Signature**: `pub fn split_text_and_special_tokens(carry: &mut String, chunk: &str) -> (Vec<String>, Vec<String>)`
*   **Description**: Parses incoming text streams, extracting complete performance tags while buffering incomplete brackets. Returns clean dialogue text and isolated tags.

#### `strip_markers`
*   **Signature**: `pub fn strip_markers(text: &str) -> String`
*   **Description**: Removes all performance tags (e.g. `<|perf:motion=wave|>`) from a text block.

#### `parse_performance_marker`
*   **Signature**: `pub fn parse_performance_marker(token: &str) -> Option<PerformanceCue>`
*   **Description**: Parses a tag string (e.g. `<|perf:expr=joy|>`) into a type-safe `PerformanceCue` enum.

#### `strip_token_envelope`
*   **Signature**: `fn strip_token_envelope(token: &str) -> Option<&str>`
*   **Description**: Trims the special marker envelope `<|perf:` and `|>` wrappers.

#### `parse_expr_marker`
*   **Signature**: `fn parse_expr_marker(rest: &str) -> Option<PerformanceCue>`
*   **Description**: Extracts the name, weight, and hold duration from an expression marker tag.

#### `parse_motion_marker`
*   **Signature**: `fn parse_motion_marker(rest: &str) -> Option<PerformanceCue>`
*   **Description**: Extracts the name and target layer from a motion marker tag.
