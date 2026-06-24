# `ene-session` — API Reference

> **Crate:** `ene-session`  
> **Role:** Conversation session management, streaming text processing, and session boundary detection.

---

## Overview

`ene-session` owns the mutable conversation state for a single chat session. It tracks history, handles streaming delta assembly, parses special tokens (emotion markers, etc.), and determines when a session should be split into memory.

A `ConversationSession` is held and driven by the `EneActor` inside `ene-core`.

---

## `ConversationSession`

The central type. Combines conversation history, memory context, and streaming state.

```rust
pub struct ConversationSession { /* opaque */ }
```

### Construction & Initialization

| Method | Signature | Description |
|--------|-----------|-------------|
| `new` | `fn new() -> Self` | Creates a fresh session with no history and a new `SessionId`. |
| `init_memory` | `fn init_memory(&mut self, store: Arc<MemoryStore>, embedder: Arc<dyn EmbeddingProvider>)` | Attaches a memory store and embedding provider to the session. Must be called before memory operations are used. |
| `load_card` | `fn load_card(&mut self, path: &str) -> Result<Vec<ResolvedExpression>, EneSessionError>` | Loads a character card from the filesystem and returns resolved expression assets. |

### History Management

| Method | Signature | Description |
|--------|-----------|-------------|
| `add_user_message` | `fn add_user_message(&mut self, input: &str)` | Appends a user turn to history. |
| `add_assistant_message` | `fn add_assistant_message(&mut self, text: &str)` | Appends an assistant turn to history. |
| `history` | `fn history(&self) -> &[(Role, String)]` | Returns the full conversation history as a slice. |

### Streaming

| Method | Signature | Description |
|--------|-----------|-------------|
| `process_delta` | `fn process_delta(&mut self, chunk: &str) -> (Vec<String>, Vec<String>)` | Feeds a streaming chunk into the buffer. Returns `(text_deltas, special_tokens)`. |
| `finalize_response` | `fn finalize_response(&mut self) -> Option<String>` | Flushes the stream buffer and returns the complete assistant message, if any. |
| `reset_display_buffer` | `fn reset_display_buffer(&mut self)` | Clears the streaming display buffer without affecting history. |

### Session Lifecycle

| Method | Signature | Description |
|--------|-----------|-------------|
| `reset_session` | `fn reset_session(&mut self) -> SessionId` | Resets history and generates a new `SessionId`. Returns the new ID. |
| `session_id` | `fn session_id(&self) -> &SessionId` | The current session's unique ID. |
| `session_started_at` | `fn session_started_at(&self) -> DateTime<Utc>` | When the current session began. |
| `session_elapsed_minutes` | `fn session_elapsed_minutes(&self) -> i64` | Minutes elapsed since session start. |
| `current_turn_count` | `fn current_turn_count(&self) -> usize` | Number of completed turns in this session. |
| `last_message_time` | `fn last_message_time(&self) -> Option<DateTime<Utc>>` | Time of the most recent message. |

### Embeddings

| Method | Signature | Description |
|--------|-----------|-------------|
| `set_pending_embedding` | `fn set_pending_embedding(&mut self, embedding: Vec<f32>)` | Stores the embedding of the current user input for the memory lookup in the next turn. |
| `set_last_input_embedding` | `fn set_last_input_embedding(&mut self, embedding: Vec<f32>)` | Stores the embedding for session boundary detection. |

### Persistence

| Method | Signature | Description |
|--------|-----------|-------------|
| `record_user_input` | `fn record_user_input(&mut self)` | Persists the pending user turn to the conversation log. |
| `record_assistant_response` | `fn record_assistant_response(&mut self)` | Persists the pending assistant turn to the conversation log. |

### Accessors

| Method | Signature | Description |
|--------|-----------|-------------|
| `card_name` | `fn card_name(&self) -> &str` | The name of the loaded character card. |
| `apply_pending_split` | `fn apply_pending_split(&mut self)` | Commits a pending session split that was prepared asynchronously. |
| `prepare_split_input` | `fn prepare_split_input(&self) -> SplitTaskInput` | Gathers the data needed to run an async split task. |

---

## Internal Structs

### `ConversationHistory`

```rust
pub struct ConversationHistory {
    /// The ordered list of (role, content) pairs.
    pub conversation_history: Vec<(Role, String)>,

    /// Maximum number of turns to keep in the active context window.
    pub max_history_turns: usize,
}
```

When `max_history_turns` is exceeded, older turns are dropped from the context sent to the LLM (but not from the persistent log).

### `MemoryContext`

```rust
pub struct MemoryContext {
    /// The attached memory store (None if memory is disabled).
    pub memory_store: Option<Arc<MemoryStore>>,

    /// The embedding provider for query encoding.
    pub embedding_provider: Option<Arc<dyn EmbeddingProvider>>,

    /// The current session's ID.
    pub session_id: SessionId,

    /// When the session started.
    pub session_started_at: DateTime<Utc>,

    /// The embedding of the most recent user input, ready for memory lookup.
    pub pending_embedding: Option<Vec<f32>>,
}
```

---

## Session Boundary Types

### `SessionBoundary`

The result of evaluating whether the current turn should start a new session.

```rust
pub enum SessionBoundary {
    /// Continue the current session.
    Continue,

    /// Split the session, creating a memory summary.
    Split(SplitReason),
}
```

### `SplitReason`

```rust
pub enum SplitReason {
    /// The session has been inactive for too long.
    Timeout { elapsed_minutes: u64 },

    /// The topic has shifted significantly (low embedding similarity).
    TopicChange { similarity: f32 },

    /// The user or system triggered a manual split.
    Manual,
}
```

### `SplitResult`

```rust
pub struct SplitResult {
    /// The reason the split was triggered.
    pub reason: SplitReason,

    /// LLM-generated summary of the completed session.
    pub summary: String,

    /// Key facts extracted from the session.
    pub key_facts: Vec<KeyFact>,

    /// The newly generated session ID for the next session.
    pub new_session_id: SessionId,
}
```

---

## Session Boundary Functions

### `check_boundary`

```rust
pub fn check_boundary(
    last_embedding: Option<&Vec<f32>>,
    last_time: Option<&DateTime<Utc>>,
    turn_count: usize,
    settings: &SessionSettings,
    user_input: &str,
    embedder: &dyn EmbeddingProvider,
) -> SessionBoundary
```

Evaluates whether the incoming user message should trigger a session split. Checks:
1. **Timeout:** Has the session been idle longer than the configured threshold?
2. **Topic change:** Is cosine similarity between the last-turn embedding and the new input below the threshold?

### `execute_split`

```rust
pub async fn execute_split(
    /* session data */
    ...
) -> Result<SplitResult, EneSessionError>
```

Runs the full split pipeline: summarizes the session history using the LLM, extracts key facts, persists the summary to the memory store, and generates a new `SessionId`.

### `spawn_split_task`

```rust
pub fn spawn_split_task(
    pending: &mut Option<PendingSplitTask>,
    input: SplitTaskInput,
)
```

Spawns `execute_split` as a background Tokio task. The result is collected later via `poll_split_result`.

### `poll_split_result`

```rust
pub fn poll_split_result(
    pending: &mut Option<PendingSplitTask>,
) -> Option<Result<SplitResult, EneSessionError>>
```

Non-blocking poll of the background split task. Returns `Some` when the task has completed, `None` if still running.

### `generate_session_id`

```rust
pub fn generate_session_id() -> SessionId
```

Generates a new unique `SessionId` (UUID-based).

### `embed_session_messages`

```rust
pub async fn embed_session_messages(
    embedder: &dyn EmbeddingProvider,
    history: &[(Role, String)],
) -> Result<Vec<f32>, EneSessionError>
```

Embeds the User and Assistant messages from history individually, then averages the per-message vectors to produce one summary vector. Non-text, empty, and non-User/Assistant messages are filtered out. The result represents the session's semantic content for boundary detection.

---

## Special Token Parsing

The LLM output stream may contain special tokens (e.g., `<|emotion:happy|>`) that drive UI effects.

### `split_text_and_special_tokens`

```rust
pub fn split_text_and_special_tokens(
    carry: &mut String,
    chunk: &str,
) -> (Vec<String>, Vec<String>)
```

Parses a streaming chunk for special tokens. The `carry` buffer holds an incomplete token boundary across chunk boundaries.

Returns `(text_deltas, special_tokens)`:
- `text_deltas`: plain text fragments to display.
- `special_tokens`: complete special tokens found in this chunk.

### `extract_emotion_from_token`

```rust
pub fn extract_emotion_from_token(token: &str) -> Option<String>
```

If `token` matches the emotion token format (`<|emotion:NAME|>`), returns `Some(NAME)`. Otherwise returns `None`.

---

## Type-Safe ID Wrappers

### `SessionId`

```rust
pub struct SessionId(String);

impl SessionId {
    pub fn new(id: impl Into<String>) -> Self;
    pub fn as_str(&self) -> &str;
    pub fn into_inner(self) -> String;
}

impl From<String> for SessionId { ... }
impl From<&str> for SessionId { ... }
```

### `CardName`

```rust
pub struct CardName(String);

impl CardName {
    pub fn new(name: impl Into<String>) -> Self;
    pub fn as_str(&self) -> &str;
    pub fn into_inner(self) -> String;
}

impl From<String> for CardName { ... }
impl From<&str> for CardName { ... }
```

Both are newtype wrappers over `String` that prevent accidentally mixing session IDs and card names at the type level.

---

## Re-exports

`ene-session` re-exports the `Truncate` trait from `ene-common` for convenience:

```rust
pub use ene_common::truncate::Truncate;
```

---

## Usage Example

```rust
use ene_session::ConversationSession;

let mut session = ConversationSession::new();

// Simulate a streaming turn
session.add_user_message("Tell me a joke");

// Feed streaming chunks as they arrive
let chunks = ["Why did the ", "chicken cross", " the road?"];
for chunk in &chunks {
    let (text, tokens) = session.process_delta(chunk);
    for t in text { print!("{}", t); }
    for t in tokens { eprintln!("[special: {}]", t); }
}

// Finalize after stream ends
if let Some(full_response) = session.finalize_response() {
    session.add_assistant_message(&full_response);
}

println!("Turn count: {}", session.current_turn_count());
```

---

## See Also

- [`ene-core`](./ene-core.md) — Drives the session through `EneActor`
- [`ene-memory`](./ene-memory.md) — Stores summaries created on split
- [`ene-common`](./ene-common.md) — `Truncate` utility trait
