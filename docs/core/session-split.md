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

`check_boundary()` evaluates two conditions:

1. **Timeout**
   - `session_elapsed_minutes() >= session_timeout_minutes`
   - AND `current_turn_count >= min_turns_before_split`

2. **Topic Change**
   - Cosine similarity between previous and current user input embeddings `< topic_change_threshold`
   - Requires at least 2 user inputs with valid embeddings
   - AND `current_turn_count >= min_turns_before_split`

## Async Lifecycle

```
User input received
    ↓
spawn_split_task() → background Tokio task
    ↓
                    check_boundary()
                        ↓
                    Continue | Split
                        ↓ (Split)
                    execute_split()
                        ↓
                    Send SplitResult via oneshot channel
    ↓
poll_split_result() → non-blocking check
    ↓ (if complete)
session.reset_session()
```

Only one split task runs at a time — calling `spawn_split_task()` when one is already pending is ignored.

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
