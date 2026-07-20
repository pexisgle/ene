# Proactive Speech / Proactive Decision Specifications

Ene monitors the user's desktop context (active window, input idle time), screen content, and pending commitments to initiate a conversational turn without a direct user prompt. This document defines the proactive gates and the LLM decision loop.

---

## 1. Data Structures

### `ActivitySnapshot` (Public / Struct)
Privacy-safe operating system context pushed from the host application:
*   `idle_seconds: Option<u64>`: Seconds since the last keyboard/mouse action.
*   `active_window_label: String`: Normalized application category (e.g. `Browser`, `CodeEditor` rather than raw window title bars).
*   `recent_change: String`: Summary of the active application change.

### `ProactiveObservation` (Public / Struct)
The observation frame containing desktop activity metrics:
*   `captured_at_unix_ms: u64`: Captured timestamp.
*   `activity: Option<ActivitySnapshot>`: Desktop interaction snap.
*   `screen_summary: Option<String>`: Text summary of current screen contents (never raw screenshots).

### `ProactiveSuppressionState` (Public / Struct)
Prevents conversational flooding (spamming the user):
*   `seconds_since_user_input: u64`: Seconds elapsed since the last user message.
*   `seconds_since_proactive: u64`: Seconds elapsed since the last proactive speech.
*   `proactive_turns_this_session: usize`: Count of proactive turns in the active session.
*   `user_turn_busy: bool`: True if the actor is processing a turn, awaiting approvals, or collecting user inputs.

### `ProactiveDecision` (Public / Struct)
Structured decision model returned by the lightweight LLM:
```rust
pub struct ProactiveDecision {
    pub should_speak: bool,
    pub confidence: f64,
    pub screen_digest: String,
    pub reason: String,
    pub topic_hint: String,
    pub urgency: ProactiveUrgency,
}
```

---

## 2. Decision Logic and Models (`mod.rs`)

#### `parse` (for ProactiveObservation)
*   **Signature**: `fn parse(raw: Option<&str>) -> Self`
*   **Description**: Safely parses raw JSON observations sent from host desktop daemons.

#### `silent` (for ProactiveDecisionResult)
*   **Signature**: `pub fn silent(reason: impl Into<String>) -> Self`
*   **Description**: Creates a negative decision result, logging the reason why Ene remained silent.

#### `allows_generation`
*   **Signature**: `pub fn allows_generation(&self, min_confidence: f64) -> bool`
*   **Description**: Checks if `should_speak` is true and the confidence exceeds `min_confidence`.

---

## 3. Deterministic Filtering Gates (`gate.rs`)

Because LLM generation is computationally expensive, a set of deterministic rules filters out invalid candidates before invoking the model.

#### `evaluate_deterministic_gates`
*   **Signature**: `pub fn evaluate_deterministic_gates(config: &ProactiveConfig, context: &ProactiveContext) -> Result<(), GateRejectReason>`
*   **Rejection Rules**:
    -   `UserTurnBusy`: Rejects if the user is typing or if approvals/tool tasks are pending.
    -   `CooldownActive`: Rejects if the elapsed time since the last proactive speech is less than `cooldown_seconds`.
    -   `SystemSessionLimitExceeded`: Rejects if the total proactive turn count for the session is exceeded.
    -   `NotIdle`: Rejects if the user has been active recently (idle time is less than `idle_seconds_required`).
    -   `UserActiveWindowEmpty` / `ActiveWindowMuted`: Rejects if the active window is empty or matches blacklisted apps.

---

## 4. Prompt Assembly (`prompt.rs`)

#### `build_decision_messages`
*   **Signature**: `pub fn build_decision_messages(context: &ProactiveContext, prompt_language: &str) -> Vec<LlmMessage>`
*   **Description**: Assembles LLM messages instructing the model to evaluate if a topic or window event warrants interruption.
*   **Prompt Order**:
    1.  `System`: Background context instructions.
    2.  `System`: Current emotional affect state and active commitments.
    3.  `System`: Recent history transcripts.
    4.  `User`: Current operating system observation.

#### `format_context_block`
*   **Signature**: `fn format_context_block(context: &ProactiveContext) -> String`
*   **Description**: Formats the active window label, idle time, and screen summary into user cues.

---

## 5. Decision Parsing & Validation (`parse.rs`)

#### `decision_schema_object`
*   **Signature**: `pub fn decision_schema_object() -> Value`
*   **Description**: Constructs the raw JSON Schema object properties defining output parameters.

#### `decision_schema`
*   **Signature**: `pub fn decision_schema() -> Value`
*   **Description**: Builds the schema envelope forcing model compliance.

#### `parse_decision_json`
*   **Signature**: `pub fn parse_decision_json(raw: &str) -> ProactiveDecision`
*   **Description**: Decodes the LLM response, stripping markdown noise and falling back to a silent default on deserialization failures.

#### `extract_json_object`
*   **Signature**: `fn extract_json_object(raw: &str) -> Option<&str>`
*   **Description**: Trims markdown braces and noise surrounding JSON output blocks.
