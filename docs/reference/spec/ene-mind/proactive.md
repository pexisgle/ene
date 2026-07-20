# Proactive Speech / Proactive Decision Specifications

Ene monitors the user's desktop context (active window, input idle time), screen content, and pending commitments to initiate a conversational turn without a direct user prompt. This document defines the proactive gates and the LLM decision loop.

---

## 1. Data Structures

### 1. Host Observations

#### `ActivitySnapshot` (Public / Struct)
Privacy-safe operating system context pushed from the host application:
*   `idle_seconds: Option<u64>`: Seconds since the last keyboard/mouse action.
*   `active_window_label: String`: Normalized application category (e.g. `Browser`, `CodeEditor` rather than raw window title bars).
*   `recent_change: String`: Summary of the active application change.

#### `ProactiveObservation` (Public / Struct)
The observation frame containing desktop activity metrics:
*   `captured_at_unix_ms: u64`: Captured timestamp.
*   `activity: Option<ActivitySnapshot>`: Desktop interaction snap.
*   `screen_summary: Option<String>`: Text summary of current screen contents (never raw screenshots).

---

### 2. Control Parameters

#### `ProactiveSuppressionState` (Public / Struct)
Prevents conversational flooding (spamming the user):
*   `seconds_since_user_input: u64`: Seconds elapsed since the last user message.
*   `seconds_since_proactive: u64`: Seconds elapsed since the last proactive speech.
*   `proactive_turns_this_session: usize`: Count of proactive turns in the active session.
*   `user_turn_busy: bool`: True if the actor is processing a turn, awaiting approvals, or collecting user inputs.

#### `ProactiveDecision` (Public / Struct)
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

## 2. Deterministic Filtering Gates (`gate.rs`)

Because LLM generation is computationally expensive, a set of deterministic rules (`evaluate_deterministic_gates`) filters out invalid candidates before invoking the model:

### Gate Rejection Reasons (`GateRejectReason`)
If any of these conditions are met, the request is skipped:
*   `UserTurnBusy`: Active turn processing is in progress.
*   `CooldownActive`: Elapsed time since the last proactive turn is less than `cooldown_seconds`.
*   `SystemSessionLimitExceeded`: The session proactive turn count exceeds configuration limits.
*   `NotIdle`: User interaction is active (idle time is less than `idle_seconds_required`).
*   `UserActiveWindowEmpty` / `ActiveWindowMuted`: The active app label is blank or matches a muted application blacklist (such as fullscreen gaming).

---

## 3. Decision Pipeline (`decide_proactive_speech`)

If the deterministic gates pass, the LLM pipeline executes:

1.  **Prompt Composition (`build_decision_messages`)**:
    -   Combines emotional valence, commitments, recent message history, active window label, and screen text summaries.
    -   Instructs the model to evaluate if a topic, promise, or desktop event warrants interrupting the user.
2.  **Schema Enforcement**:
    -   Leverages `decision_schema()` to force a structured JSON output fitting `ProactiveDecision`.
3.  **Confidence Check**:
    -   Checks if `should_speak` is true and `confidence` exceeds `min_confidence`. If verified, the actor launches a proactive turn using the provided `topic_hint`.
