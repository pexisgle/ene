# Streaming Engine

`run_ai_with_tools()` in `ene-core` executes streaming LLM conversations with tool calling loops.

## `AiStreamEvent`

The primary event type yielded by the streaming pipeline:

```rust
pub enum AiStreamEvent {
    TextDelta(String),                                    // Text fragment
    SpecialToken(String),                                 // e.g. <|emo:happy|>
    ToolCallStart { name: String, arguments: String },    // Tool invocation starts
    ToolCallResult { name: String, result: String },      // Tool execution result
    PermissionRequired { request_id, action, target, description }, // Phase 2
    TaskProgress { task_id, step, total_steps, description },       // Phase 2
    SessionSplit { summary: String, reason: String },               // Phase 2
    Finished,                                             // Response complete
    Error(String),                                        // Error details
}
```

## `run_ai_with_tools()` Flow

```rust
pub async fn run_ai_with_tools(
    settings: &EneSettings,
    session: &ConversationSession,
    user_input: &str,
    registry: Arc<dyn ToolRegistry>,
) -> Result<impl Stream<Item = AiStreamEvent>, AiCoreError>
```

### Step-by-Step

1. **Preparation** — Resolve `base_url`/`api_key`, verify card loaded, save user input to `conversation_logs` (async, if memory enabled)

2. **Memory Search**
   - `get_all_keyfacts()` → existing user facts
   - If Tool RAG enabled: embed user input → `store.search_tools()` → relevant tools
   - `search_summaries()` → recalled past conversation summaries

3. **Message Construction**
   - `build_messages()` → full message array (system prompt, examples, recalled summaries, history, protocol, user input)
   - `build_tools()` → `ToolDefinition` list → OpenAI function calling format

4. **Main Loop** (up to `max_tool_call_rounds` iterations)

   ```
   POST chat/completions (stream)
       ↓
   TextDelta events emitted
       ↓
   ToolCallChunk accumulation
       ↓
   After stream ends, if tool_calls exist:
     ├── ToolCallStart event
     ├── registry.call_tool(name, args)  (30s timeout)
     ├── Screenshot results → image message conversion
     ├── ToolCallResult event
     └── Continue loop
   Otherwise:
     └── Save assistant log, Finished event
   ```

5. **Post-Processing**
   - `AiStreamEvent::Finished` emitted
   - History finalization is caller's responsibility (`session.finalize_response()`)

## Tool Call Accumulation

Streaming tool calls arrive in chunks that must be accumulated:

```rust
fn accumulate_tool_calls(chunks: &mut Vec<ToolCallChunk>, delta: &[ToolCallChunk])
fn finalize_tool_calls(chunks: Vec<ToolCallChunk>) -> Vec<ToolCalls>
```

Each chunk is identified by its `index` field. `function.arguments` strings are concatenated across chunks.

## Screenshot Handling

If a tool result contains `{"type":"screenshot","data":"data:image/png;base64,..."}`, the base64 data is extracted and converted into a `ChatCompletionRequestMessage::UserMessage` with an image URL for the LLM's next API call.
