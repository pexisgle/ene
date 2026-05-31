# Session Management

`ConversationSession` is the central container for conversation state.

## Structure

```rust
pub struct ConversationSession {
    pub(crate) history: ConversationHistory,   // crate-private
    pub display: DisplayState,
    pub memory: MemoryContext,
    pub(crate) state: SessionState,            // crate-private
    pub character_card: Option<CharacterCardV3>,
    current_card_path: String,                 // private
}
```

### Sub-Structures

```rust
pub struct ConversationHistory {
    pub conversation_history: Vec<(Role, String)>,
    pub max_history_turns: usize,  // Default 20
}

pub struct DisplayState {
    pub display_buffer: String,   // Accumulated streaming text
    pub token_carry: String,      // Partial token across chunks
}

pub struct MemoryContext {
    pub memory_store: Option<Arc<MemoryStore>>,
    pub embedding_provider: Option<Arc<dyn EmbeddingProvider>>,
    pub session_id: String,
    pub session_started_at: DateTime<Utc>,
    pub pending_embedding: Option<Vec<f32>>,
}

pub struct SessionState {
    pub last_input_embedding: Option<Vec<f32>>,
    pub last_message_time: Option<DateTime<Utc>>,
    pub current_turn_count: usize,
}
```

## Key Methods

| Method | Description |
|--------|-------------|
| `new()` | Creates empty session with auto-generated `session_id` |
| `init_memory(store, embedder)` | Attaches memory store and embedding provider |
| `load_card(path)` | Loads character card, merges `character_settings.json`, clears history |
| `add_user_message(input)` | Appends user message, auto-trims history |
| `add_assistant_message(text)` | Appends assistant response, auto-trims |
| `process_delta(chunk)` | Splits stream chunk into text/special tokens |
| `finalize_response()` | Commits display buffer as assistant message |
| `reset_session()` | Clears all state, generates new `session_id` |
| `card_name()` | Returns character name or `"default"` |
| `session_elapsed_minutes()` | Minutes since session start |

## CharacterCardV3

Defined in `ene_config` — the V3 character card format:

| Field | Description |
|-------|-------------|
| `spec` / `spec_version` | Format version identifiers |
| `data.name` | Character name |
| `data.nickname` | Alternate display name (preferred over `name` when non-empty) |
| `data.description` | Character description text |
| `data.personality` | Personality description |
| `data.scenario` | Scenario setting |
| `data.system_prompt` | System prompt override |
| `data.first_mes` | First message when card is loaded |
| `data.alternate_greetings` | Alternative greeting messages |
| `data.mes_example` | Example conversation |
| `data.post_history_instructions` | Post-history instructions (PHI) |
| `data.creator_notes` | Notes from the card creator |
| `data.tags` | Tags / categories for discovery |
| `data.creator` | Card creator name |
| `data.character_version` | Version string for this character |
| `data.extensions` | Extensions (expression definitions, config) |
| `data.assets` | Asset references (VRM models, images) |
| `data.character_book` | Optional lorebook for world-building context |
| `data.source` | Attribution sources |
| `data.group_only_greetings` | Alternative greetings for group chats |
| `data.creation_date` | Unix timestamp of creation |
| `data.modification_date` | Unix timestamp of last modification |
