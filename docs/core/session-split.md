# Automatic Session Splitting

Sessions are automatically split to manage LLM context window limits and adapt to topic changes.

## Split Reasons

```rust
pub enum SplitReason {
    Timeout { elapsed_minutes: u64 },
    TopicChange { similarity: f32 },
    Manual,
}
```

## Trigger Conditions

`check_boundary()` computes a **composite split score** based on four normalized factors:

1. **Time Elapsed**: `minutes / timeout_minutes`
2. **Topic Distance**: `1.0 - cosine_similarity` between previous and current user embeddings
3. **Context Pressure**: `history_len / max_history`
4. **Turn Count**: `current_turn_count / 100`

The total score is calculated as:
`time_component + topic_component + context_component + turn_component`

A split is triggered when the total score exceeds the `threshold` (default `0.65`). The dominant contributing factor determines the `SplitReason`.

*Note: A minimum number of turns (`min_turns_before_split`) is required before any split can occur.*

## Lifecycle

### Automatic Split (during streaming)

```
User sends input
  ↓
Actor: check_and_perform_split(user_input)
  ↓
check_boundary() → Continue | Split(SplitReason)
  ↓ (Split)
spawn_split_task() → background Tokio task
  ↓
  execute_split()
    ↓
  Send SplitResult via oneshot channel
  ↓
Actor: apply_pending_split() on next Run
  ↓
session.reset_session() + new session_id
```

Only one split task runs at a time — calling `spawn_split_task()` when one is already pending is ignored.

### Manual Split (via /session split command)

```
User: /session split
  ↓
CLI sends EneCommand::ManualSplit { reply }
  ↓
Actor: handle_manual_split()
  ├── Validates: non-empty history, memory enabled, embedder available
  ├── Creates LLM provider
  ├── Calls execute_split() with SplitReason::Manual
  ├── Emits EneEvent::SessionSplit
  ├── Resets session with new session_id
  └── Returns SplitResult via oneshot
  ↓
CLI displays summary + key facts
```

## `execute_split()` Steps

1. Save full conversation history to `conversation_logs`
2. Retrieve existing key facts
3. Call `summarize_conversation()` → LLM generates structured summary + topics + key facts
4. `embed_session_messages()` → individual message embeddings → max-pooling → single vector
5. `insert_summary()` → save summary + key facts to `MemoryStore`
6. Return new session ID

## Max-Pooling Embedding

`embed_session_messages()` embeds each user/assistant message individually, then applies max-pooling across dimensions (takes the maximum value for each element). This prevents low-information messages (greetings, acknowledgments) from diluting the session's semantic signal. The resulting vector is normalized.

## Result Polling

`poll_split_result()` performs a non-blocking check on the oneshot receiver. When a result arrives, the caller performs a session reset with the new session ID. During the next message cycle, a fresh `ConversationSession` begins.
