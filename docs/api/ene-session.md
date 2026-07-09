# `ene-session` — API Reference

> **Crate:** `ene-session`
> **Role:** Conversation session management, streaming text processing, session boundary detection, and character-driven expression state.

---

## Overview

`ene-session` owns the mutable conversation state for a single chat session. It tracks history, assembles streaming deltas, parses special tokens (emotion markers), scores and executes session boundary splits, and tracks expression hysteresis for the character's avatar.

A `ConversationSession` is held and driven by the `EneActor` inside `ene-core`. All session-split work is asynchronous and runs as a background `tokio` task so it never blocks the turn loop — the actor polls for completion and applies the result once it lands.

---

## `ConversationSession`

The central type. Combines conversation history, memory context, streaming display state, and expression tracking.

```rust
pub struct ConversationSession {
    pub(crate) history: ConversationHistory,
    pub display: DisplayState,
    pub memory: MemoryContext,
    pub(crate) state: SessionState,
    pub character_card: Option<CharacterCardV3>,
    // current_card_path: String (private)
}
```

### Construction & Initialization

| Method | Signature | Description |
|--------|-----------|-------------|
| `new` | `pub fn new() -> Self` | Creates a fresh session with no history and a new `SessionId`. |
| `init_memory` | `pub fn init_memory(&mut self, store: Arc<MemoryStore>, embedder: Arc<dyn EmbeddingProvider>)` | Attaches a memory store and embedding provider. Must be called before any memory-backed operation. |
| `load_card` | `pub fn load_card(&mut self, path: &str) -> Result<Vec<ResolvedExpression>, EneSessionError>` | Loads a character card from disk and returns its resolved expression assets. |

### History Management

| Method | Signature | Description |
|--------|-----------|-------------|
| `add_user_message` | `pub fn add_user_message(&mut self, input: &str)` | Appends a user turn to history. |
| `add_assistant_message` | `pub fn add_assistant_message(&mut self, text: &str)` | Appends an assistant turn to history. |
| `history` | `pub fn history(&self) -> &[(Role, String)]` | Returns the full in-memory conversation history. |
| `trim_history_keep_last` | `pub fn trim_history_keep_last(&mut self, keep: usize)` | Trims in-memory history, keeping only the last `keep` messages. |

### Streaming

| Method | Signature | Description |
|--------|-----------|-------------|
| `process_delta` | `pub fn process_delta(&mut self, chunk: &str) -> (Vec<String>, Vec<String>)` | Feeds a streaming chunk into the display buffer, splitting it into text deltas and special tokens (e.g. `<|emo:happy|>`). Returns `(text_deltas, special_tokens)`. |
| `finalize_response` | `pub fn finalize_response(&mut self) -> Option<String>` | Flushes any remaining token carry, commits the buffered text as an assistant message, and returns any lingering token fragment. |
| `reset_display_buffer` | `pub fn reset_display_buffer(&mut self)` | Clears the streaming display buffer without touching history. |

### Session Lifecycle

| Method | Signature | Description |
|--------|-----------|-------------|
| `reset_session` | `pub fn reset_session(&mut self) -> SessionId` | Resets history, display state, and turn count; generates and returns a new `SessionId`. |
| `session_id` | `pub fn session_id(&self) -> &SessionId` | The current session's unique ID. |
| `session_started_at` | `pub fn session_started_at(&self) -> DateTime<Utc>` | When the current session began. |
| `session_elapsed_minutes` | `pub fn session_elapsed_minutes(&self) -> i64` | Minutes elapsed since session start. |
| `current_turn_count` | `pub fn current_turn_count(&self) -> usize` | Number of completed turns in this session. |
| `last_message_time` | `pub fn last_message_time(&self) -> Option<DateTime<Utc>>` | Time of the most recent message. |
| `card_name` | `pub fn card_name(&self) -> &str` | Name of the loaded character card. |

### Embeddings

| Method | Signature | Description |
|--------|-----------|-------------|
| `set_pending_embedding` | `pub fn set_pending_embedding(&mut self, embedding: Vec<f32>)` | Stores the embedding of the current user input for next turn's memory lookup. |
| `set_last_input_embedding` | `pub fn set_last_input_embedding(&mut self, embedding: Vec<f32>)` | Stores the embedding used for topic-change boundary detection. |

### Turn Tracking

| Method | Signature | Description |
|--------|-----------|-------------|
| `record_user_input` | `pub fn record_user_input(&mut self)` | Increments `current_turn_count` and sets `last_message_time`. **Does not** persist anything to the memory log — raw logging happens separately, inside `execute_split`. |
| `record_assistant_response` | `pub fn record_assistant_response(&mut self)` | Same bookkeeping as `record_user_input`, called after the assistant's turn. |

### Expression Tracking

Backs the character's expression *arbiter* with in-session hysteresis, so expressions don't flicker between visually similar states turn-to-turn.

| Method | Signature | Description |
|--------|-----------|-------------|
| `expression_elapsed` | `pub fn expression_elapsed(&self) -> Option<std::time::Duration>` | Time elapsed since the last expression change, or `None` if no change has been recorded yet this session. |
| `record_expression_change` | `pub fn record_expression_change(&mut self, name: &str)` | Records a newly resolved expression and its timestamp for hysteresis tracking. |
| `last_resolved_expression` | `pub fn last_resolved_expression(&self) -> &str` | The last resolved expression name for this session (empty string if none yet). |
| `expression_context` | `pub fn expression_context<'a>(&'a self, affect: &'a AffectState) -> (Cow<'a, str>, Option<std::time::Duration>)` | Returns `(previous_expression, elapsed)` for arbiter hysteresis. Falls back to the persisted `AffectState::last_expression` / `updated_at` when the in-session tracker is empty (e.g. right after a restart). |

### Session Split Integration

| Method | Signature | Description |
|--------|-----------|-------------|
| `prepare_split_input` | `pub fn prepare_split_input(&self, config: &EneConfig, user_input: &str, user_name: &str, provider: Arc<dyn LlmProvider>) -> Option<SplitTaskInput>` | Gathers everything needed to run a split task in the background. Returns `None` if memory hasn't been initialized. |
| `mark_split_pending` | `pub fn mark_split_pending(&mut self)` | Records the current history length as the snapshot boundary for an in-flight split. |
| `is_split_pending` | `pub fn is_split_pending(&self) -> bool` | Whether a split snapshot boundary is currently recorded. |
| `apply_split_result` | `pub fn apply_split_result(&mut self, split: &SplitResult)` | Applies a completed split: truncates history up to the snapshot boundary (`split.snapshot_len`), preserving the triggering turn and anything appended while the split was running; rotates to `split.new_session_id`; clears the pending marker. |
| `clear_split_pending` | `pub fn clear_split_pending(&mut self)` | Clears the pending marker without applying a result — used for non-fatal split errors like `EneSessionError::SplitNotNeeded`. |
| `apply_pending_split` | `pub fn apply_pending_split(&mut self, pending_split: &mut Option<PendingSplitTask>) -> Option<Result<SplitResult, EneSessionError>>` | Polls a background split task via `poll_split_result`; on success, resets the session (**clears all history**) and adopts the new session ID. Prefer `mark_split_pending` + `apply_split_result` when you need to preserve in-flight messages — see note below. |

> **Note:** `apply_pending_split` and `apply_split_result` implement two different strategies for consuming a `SplitResult`. `apply_pending_split` unconditionally calls `reset_session()`, discarding all history. `apply_split_result` instead truncates only up to `snapshot_len`, keeping messages that arrived after the split snapshot was taken. New integrations should prefer the `mark_split_pending` / `apply_split_result` pair.

---

## Supporting Session Types

### `ConversationHistory`

```rust
/// Manages the conversation history with automatic trimming.
pub struct ConversationHistory {
    pub conversation_history: Vec<(Role, String)>,
    pub max_history_turns: usize,
}
```

When `max_history_turns` is exceeded, the oldest turns are trimmed from the in-memory context sent to the LLM (the persistent conversation log in `ene-memory` is unaffected).

### `DisplayState`

```rust
/// Holds the current display buffer and partial token carry-over.
#[derive(Clone, Debug, Default)]
pub struct DisplayState {
    pub display_buffer: String,
    pub token_carry: String,
}
```

### `MemoryContext`

```rust
pub struct MemoryContext {
    pub memory_store: Option<Arc<MemoryStore>>,
    pub embedding_provider: Option<Arc<dyn EmbeddingProvider>>,
    pub session_id: SessionId,
    pub session_started_at: DateTime<Utc>,
    pub pending_embedding: Option<Vec<f32>>,
    /// Cached hash of the last synced CCv3 character memory index.
    pub ccv3_memory_hash: Option<u64>,
}
```

### `SessionState` *(private field, exposed via accessors)*

```rust
#[derive(Clone, Debug, Default)]
pub struct SessionState {
    pub last_input_embedding: Option<Vec<f32>>,
    pub last_message_time: Option<DateTime<Utc>>,
    pub current_turn_count: usize,
    /// History length at the moment a split snapshot was taken, while a split is in flight.
    pub pending_split_snapshot_len: Option<usize>,
    pub last_resolved_expression: String,
    pub last_expression_changed_at: Option<DateTime<Utc>>,
}
```

---

## Session Boundary Types

### `SessionBoundary`

```rust
#[derive(Debug)]
pub enum SessionBoundary {
    /// The session should continue without splitting.
    Continue,
    /// The session should be split for the given reason.
    Split(SplitReason),
}
```

### `SplitReason`

Five variants — flags the boundary detector's trigger for observability and prompt-facing messaging:

```rust
#[derive(Debug, Clone)]
pub enum SplitReason {
    /// Inactivity timeout exceeded.
    Timeout { elapsed_minutes: u64 },
    /// Topic shifted significantly (low embedding similarity).
    TopicChange { similarity: f32 },
    /// Context length pressure — history is approaching its cap.
    ContextPressure { context_ratio: f32 },
    /// A high composite score across multiple factors triggered the split.
    Composite { score: f32 },
    /// The user or system requested a manual split.
    Manual,
}
```

`SplitReason` implements `Display` for human-readable logging/UI messages.

### `SplitResult`

```rust
#[derive(Debug, Clone)]
pub struct SplitResult {
    pub reason: SplitReason,
    pub summary: String,
    pub key_facts: Vec<KeyFact>,
    pub new_session_id: SessionId,
    /// Number of history entries that were in the snapshot passed to the summarizer.
    /// `apply_split_result` discards entries before index `snapshot_len - 1` and keeps
    /// the rest, so messages that arrived while the split was running are preserved.
    pub snapshot_len: usize,
}
```

---

## Session Boundary Detection & Scoring

### `check_boundary`

```rust
pub async fn check_boundary(
    last_input_embedding: Option<&Vec<f32>>,
    last_message_time: Option<DateTime<Utc>>,
    current_turn_count: usize,
    history_len: usize,
    settings: &SessionConfig,
    user_input: &str,
    embedder: &dyn EmbeddingProvider,
) -> SessionBoundary
```

Async, and returns `SessionBoundary` directly (not a `Result`). Embeds `user_input`, computes a composite split score via `compute_split_score`, and returns `SessionBoundary::Continue` when:
- `settings.auto_split` is disabled, or
- fewer than `settings.min_turns_before_split` turns have occurred, or
- the composite score is below `settings.split_weights.threshold`.

### `compute_split_score` / `SplitScore`

```rust
#[must_use]
pub fn compute_split_score(
    time_elapsed_minutes: f64,
    topic_similarity: Option<f32>,
    context_ratio: f32,
    turn_count: usize,
    config: &SessionConfig,
) -> SplitScore
```

```rust
/// Decomposed components of a split score, for diagnostics.
#[derive(Debug, Clone)]
pub struct SplitScore {
    /// Combined weighted score. Split triggers once this reaches `config.split_weights.threshold`.
    pub total: f32,
    pub time_component: f32,
    pub topic_component: f32,
    pub context_component: f32,
    pub turn_component: f32,
}
```

Formula: `total = time * time_factor + topic * topic_distance + context * context_pressure + turn_count * turn_factor`, where `topic_distance = 1 − cosine_similarity` and `context_pressure = history_len / max_history`.

### `SplitScoreWeights`

```rust
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct SplitScoreWeights {
    /// Default 0.35.
    pub time: f32,
    /// Default 0.40.
    pub topic: f32,
    /// Default 0.20.
    pub context: f32,
    /// Default 0.05.
    pub turn_count: f32,
    /// Score threshold above which a split triggers. Default 0.65.
    pub threshold: f32,
}
```

### `execute_split`

```rust
pub async fn execute_split(
    history: &[(Role, String)],
    session_id: &str,
    card_name: &str,
    user_name: &str,
    store: &Arc<MemoryStore>,
    embedder: &Arc<dyn EmbeddingProvider>,
    provider: &dyn LlmProvider,
    reason: SplitReason,
) -> Result<SplitResult, EneSessionError>
```

Runs the full split pipeline: persists the conversation history to `conversation_logs` via `MemoryStore::insert_log`, summarizes it with the LLM (`ene_memory::summarize_conversation`), persists the summary and key facts, and generates a new `SessionId`.

### Background task orchestration

| Function | Signature | Description |
|----------|-----------|-------------|
| `spawn_split_task` | `pub fn spawn_split_task(pending_split: &mut Option<PendingSplitTask>, input: SplitTaskInput)` | Spawns a `tokio` task that calls `check_boundary` then `execute_split` (or completes with `Err(EneSessionError::SplitNotNeeded)`). No-op if a split is already pending. |
| `poll_split_result` | `pub fn poll_split_result(pending_split: &mut Option<PendingSplitTask>) -> Option<Result<SplitResult, EneSessionError>>` | Non-blocking `try_recv` on the task's oneshot channel. `None` while still running; re-stores the task on `Empty`. |
| `generate_session_id` | `pub fn generate_session_id() -> SessionId` | `SessionId::from(format!("session_{}", Uuid::new_v4()))`. |

```rust
pub struct SplitTaskInput {
    pub last_input_embedding: Option<Vec<f32>>,
    pub last_message_time: Option<DateTime<Utc>>,
    pub current_turn_count: usize,
    /// Current in-memory history length in individual messages (not turns).
    pub history_len: usize,
    pub user_input: String,
    pub session_config: SessionConfig,
    pub provider: Arc<dyn LlmProvider>,
    pub history: Vec<(Role, String)>,
    pub session_id: SessionId,
    pub card_name: CardName,
    pub user_name: String,
    pub store: Arc<MemoryStore>,
    pub embedder: Arc<dyn EmbeddingProvider>,
}

pub struct PendingSplitTask {
    // rx: oneshot::Receiver<Result<SplitResult, EneSessionError>> (private)
}
```

### `embed_session_messages`

```rust
pub async fn embed_session_messages(
    embedder: &dyn EmbeddingProvider,
    history: &[(Role, String)],
) -> Result<Vec<f32>, EneSessionError>
```

Embeds every `User`/`Assistant` message individually (skipping empty or non-text turns), then combines them via **max-pooling** — taking the largest value per dimension across all message vectors, followed by L2 normalization — rather than averaging. Max-pooling adopts the strongest signal per dimension and prevents low-information turns (e.g. greetings) from diluting the session's semantic fingerprint. Embedding each message individually also avoids hitting the embedding model's `max_length` limit on long sessions. Returns a zero vector for empty history.

---

## Special Token Parsing

The LLM output stream may contain special tokens (currently: emotion markers) that drive UI-side effects like avatar expression changes.

### `split_text_and_special_tokens`

```rust
pub fn split_text_and_special_tokens(
    carry: &mut String,
    chunk: &str,
) -> (Vec<String>, Vec<String>)
```

Splits a streaming chunk into plain-text deltas and complete special tokens of the form `<|...|>`. `carry` holds an incomplete token (or a lone trailing `<`) across chunk boundaries — pass the same `carry` buffer on every call for a given stream.

### `extract_emotion_from_token`

```rust
#[must_use]
pub fn extract_emotion_from_token(token: &str) -> Option<String>
```

Emotion tokens use the format **`<|emo:name|>`** (case-insensitive on the `emo` prefix). Returns `Some(name)` (lowercased, trimmed) for a valid emotion token, `None` for anything else (other token kinds, empty names, or plain text).

```rust
assert_eq!(extract_emotion_from_token("<|emo:happy|>"), Some("happy".to_string()));
assert_eq!(extract_emotion_from_token("<|act:wave|>"), None);
```

---

## Type-Safe ID Wrappers

### `SessionId` / `CardName`

Newtype wrappers over `String`, generated by an internal `define_id_type!` macro, that prevent accidentally mixing session IDs and card names at the type level.

```rust
pub struct SessionId(String);
pub struct CardName(String);

impl SessionId /* and CardName */ {
    pub fn new(id: impl Into<String>) -> Self;
    pub fn as_str(&self) -> &str;
    pub fn into_inner(self) -> String;
}

impl From<String> for SessionId /* and CardName */ { /* ... */ }
impl From<&str> for SessionId /* and CardName */ { /* ... */ }
```

Both derive `Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize` and implement `Display`.

---

## Configuration

### `SessionConfig`

```rust
pub struct SessionConfig {
    /// Whether to enable automatic session splitting. Default `true`.
    pub auto_split: bool,
    /// Minutes before the time factor reaches its full contribution. Default `30`.
    pub timeout_minutes: u64,
    /// Max conversation turns kept in the in-memory history window. Default `20`.
    pub max_history_turns: usize,
    /// Minimum turns required before a split may trigger. Default `3`.
    pub min_turns_before_split: usize,
    /// Max summaries injected into the prompt. Default `3`.
    pub recall_limit: usize,
    /// Embedding similarity threshold used by `search_summaries`. Default `0.5`.
    pub similarity_threshold: f32,
    pub split_weights: SplitScoreWeights,
    pub summarization: SummarizationConfig,
}
```

Loaded via `ene_config::define_config!` under the `session` settings key.

### `SummarizationConfig`

```rust
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default, PartialEq)]
pub struct SummarizationConfig {
    /// Model used for summarization; falls back to the chat model when empty.
    pub model: String,
    /// Base URL used for summarization; falls back to the chat base URL when empty.
    pub base_url: String,
}

impl SummarizationConfig {
    #[must_use]
    pub fn resolve_summarization_model(&self, fallback_model: &str) -> String;
    pub fn resolve_summarization_base_url(&self, fallback_url: &str) -> Result<String, ene_config::ConfigError>;
}
```

---

## Errors: `EneSessionError`

```rust
#[derive(Error, Debug)]
pub enum EneSessionError {
    /// Split evaluation determined a split is not (yet) needed.
    #[error("Split not needed")]
    SplitNotNeeded,
    /// The split task's oneshot channel closed unexpectedly.
    #[error("Task channel closed")]
    ChannelClosed,
    #[error(transparent)]
    Config(#[from] ene_config::EneConfigError),
    #[error("Embedding error: {0}")]
    Embedding(String),
    #[error(transparent)]
    Memory(#[from] ene_memory::EneMemoryError),
}
```

---

## Re-exports

`ene-session` re-exports several convenience items at the crate root, including `Truncate` from `ene-common`:

```rust
pub use ene_common::truncate::Truncate;
```

Also re-exported: `SessionConfig`, `SplitScoreWeights`, `SummarizationConfig`; `CharacterAsset`, `CharacterCardData`, `CharacterCardV3`, `ExpressionDefinition`, `ResolvedExpression`, `expand_cbs_macros`, `resolve_expressions` (from `ene-config`); `Role` (from `ene-provider`); `EneSessionError`; `ConversationSession`; `PendingSplitTask`, `SessionBoundary`, `SplitReason`, `SplitResult`, `SplitScore`, `SplitTaskInput`, `check_boundary`, `compute_split_score`, `execute_split`, `generate_session_id`, `poll_split_result`, `spawn_split_task`; `extract_emotion_from_token`, `split_text_and_special_tokens`; `CardName`, `SessionId`.

`embed_session_messages`, `ConversationHistory`, `DisplayState`, and `SessionState` are public but only reachable via their owning modules (`session_split::embed_session_messages`, `session::*`), not re-exported at the crate root.

---

## Usage Example

```rust,no_run
use ene_session::ConversationSession;

fn main() {
    let mut session = ConversationSession::new();

    // Simulate a streaming turn.
    session.add_user_message("Tell me a joke");
    session.record_user_input();

    let chunks = ["Why did the ", "chicken cross", " the road?", "<|emo:happy|>"];
    for chunk in &chunks {
        let (text, tokens) = session.process_delta(chunk);
        for t in text {
            print!("{t}");
        }
        for t in tokens {
            if let Some(emotion) = ene_session::extract_emotion_from_token(&t) {
                session.record_expression_change(&emotion);
                eprintln!("[emotion: {emotion}]");
            }
        }
    }

    // Finalize after the stream ends.
    if let Some(full_response) = session.finalize_response() {
        session.add_assistant_message(&full_response);
        session.record_assistant_response();
    }

    println!("Turn count: {}", session.current_turn_count());
    println!("Last expression: {}", session.last_resolved_expression());
}
```

---

## See Also

- [`ene-core`](./ene-core.md) — Drives the session through `EneActor`; polls `apply_pending_split` and applies expression changes
- [`ene-memory`](./ene-memory.md) — Stores summaries and logs created on split; backs `AffectState` fallback in `expression_context`
- [`ene-provider`](./ene-provider.md) — `LlmProvider`, `EmbeddingProvider`, `Role`, `LlmMessage`
- [`ene-config`](./ene-config.md) — `SessionConfig`, character card types, CBS macro expansion
- [`ene-common`](./ene-common.md) — `Truncate` utility trait
