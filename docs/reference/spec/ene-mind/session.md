# `ConversationSession` & Session State Specifications

This document defines Ene's session state manager, including conversation history storage, in-memory string buffering, character card loading, and inline `<|perf:…|>` cue parsing.

---

## 1. Data Structures

### `ConversationSession` (Public / Struct)
The central in-memory state keeper representing the active conversational thread:
*   **Fields**:
    -   `character_card: Option<CharacterCardV3>`: The parsed character card data.
    -   `history: Vec<HistoryEntry>`: The chronological exchange of messages between the user and assistant.
    -   `display_buffer: String`: Plain conversational text stripped of special tokens and control sequences, queued for the UI.
    -   `memory: MemoryContext`: Holds references to the sqlite store and embedding providers.
    -   `session_id: SessionId`: A type-safe wrapper around the active session's UUID.
*   **Core Methods**:
    -   `set_card(&mut self, card: &CharacterCardV3)`: Binds a character card to the session, initializing memory metadata hashes.
    -   `add_user_message(&mut self, text: String)`: Appends a `User` role message to history.
    -   `add_assistant_message(&mut self, text: String)`: Appends an `Assistant` role message to history.

---

## 2. Inline Special Tokens & Performance Cues (`special_token.rs`)

If the appraisal emotion engine is disabled, the LLM outputs inline tags to trigger mascot animations:

### 1. Tag Syntax
*   Expressions: `<|perf:expr=EXPRESSION_NAME|>`
*   Motions: `<|perf:motion=MOTION_NAME|>`

### 2. Utility Functions

#### `split_text_and_special_tokens`
*   **Signature**: `pub fn split_text_and_special_tokens(text: &str) -> (String, Vec<String>)`
*   **Description**: Splits incoming text chunks into plain conversational dialogue and a vector of matched special token tags. Plain text is written to `display_buffer` and routed to the UI via `TextDelta`.

#### `parse_performance_marker`
*   **Signature**: `pub fn parse_performance_marker(marker: &str) -> Option<PerformanceCue>`
*   **Description**: Parses a special tag string (e.g. `<|perf:expr=joy|>`) into a type-safe `PerformanceCue` struct (setting `PerfKind::Expression` and payload `joy`). Returns `None` on parse failure.

#### `strip_markers`
*   **Signature**: `pub fn strip_markers(text: &str) -> String`
*   **Description**: Removes all performance tags from a string using regex. Used to clean assistant responses before saving them to the conversation database logs.

---

## 3. Character Card V3 (CCv3) Integration

Ene imports character definitions using standard CCv3 cards:

*   **CBS Macro Expansion (`expand_cbs_macros`)**:
    Replaces occurrences of `{{char}}` and `{{user}}` in card descriptions and prompts with the character's display name and user's configuration name.
*   **Expression Resolution (`resolve_expressions`)**:
    Parses expression blendshapes (VRM shape weights mapped to expressions like joy or surprise) and resolves them into a `ResolvedExpression` catalog for the avatar renderer.
