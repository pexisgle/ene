# `EmotionEngine` / PAD Emotion State Specifications

This document defines Ene's emotional state engine, combining Pleasure-Arousal-Dominance (PAD) metrics, deterministic appraisals, temporal decay calculations, and asynchronous LLM classifier loops.

---

## 1. Struct Definition & Main Turn Methods

### `EmotionEngine` (Public / Facade)
A stateless execution pipeline coordinating emotion appraisals, temporal decays, proposals, and classifications.

#### `update_turn`
*   **Signature**: `pub fn update_turn(&self, config: &EmotionConfig, input: &mut TurnAffectInput<'_>) -> AffectUpdateResult`
*   **Process**:
    1.  Calculates and applies PAD decay via `apply_decay` on `input.elapsed_since_update` relative to the half-life configuration.
    2.  Applies deterministic text appraisals via `apply_appraisal`.
    3.  If a post-turn classifier proposal is present, blends it using confidence-weighted linear interpolations via `merge_classifier_proposal`.
    4.  Ensures PAD metrics are clamped within their boundaries.
    5.  Recalculates mood labels via `compute_mood_label` and returns the update summary.

#### `merge_classifier_proposal`
*   **Signature**: `fn merge_classifier_proposal(state: &mut AffectState, proposal: &AffectProposal, min_confidence: f32) -> Option<AffectUpdateReason>`
*   **Description**: Blends LLM appraisal proposals (valence, arousal, irritation, affinity) into the active state if confidence exceeds `min_confidence`.

#### `apply_weighted_blend`
*   **Signature**: `fn apply_weighted_blend(state: &mut AffectState, field: &'static str, target: f32, weight: f32, deltas: &mut Vec<AffectDelta>)`
*   **Description**: Linearly interpolates values using:
    $$X_{\text{new}} = (X_{\text{target}} - X_{\text{current}}) \times w + X_{\text{current}}$$
    where $w$ is the confidence parameter.

#### `compute_mood_label`
*   **Signature**: `pub fn compute_mood_label(state: &AffectState) -> String`
*   **Description**: Maps continuous coordinates to discrete states:
    *   `Joyful`: Valence > 0.3, Arousal > 0.0
    *   `Relaxed`: Valence > 0.2, Arousal <= 0.0
    *   `Anxious`: Valence < -0.2, Arousal > 0.2
    *   `Depressed`: Valence < -0.2, Arousal <= 0.2
    *   `Hostile`: Irritation > 0.5
    *   `Neutral`: Default baseline fallback.

---

## 2. Temporal Decay Functions (`decay.rs`)

#### `apply_decay`
*   **Signature**: `pub fn apply_decay(state: &mut AffectState, half_life_minutes: f64, elapsed: Duration) -> Option<AffectUpdateReason>`
*   **Description**: Decays pleasure (valence), excitement (arousal), irritation, and fatigue back to neutral baselines. Uses:
    $$V_{\text{new}} = V_{\text{old}} \times e^{-\lambda t}$$
    where $\lambda = \frac{\ln(2)}{\text{decay\_half\_life\_minutes}}$. Pinned affinity does not decay.

---

## 3. Deterministic Appraisal Functions (`appraisal.rs`)

#### `apply_appraisal`
*   **Signature**: `pub fn apply_appraisal(state: &mut AffectState, user_message: &str, recent_turn_count: usize) -> Vec<AffectUpdateReason>`
*   **Process**:
    1.  Tokenizes user messages into ASCII words.
    2.  Spikes arousal if messages are long, typed in caps, or contain multiple exclamation marks ("!!").
    3.  Accrues fatigue periodically based on `recent_turn_count`.
    4.  Increases valence and affinity if gratitude strings are matched.
    5.  Increases irritation and lowers valence if insults or aggressive keywords match.
    6.  Returns update reasons.

#### `ascii_tokens`
*   **Signature**: `fn ascii_tokens(text: &str) -> Vec<String>`
*   **Description**: Strips non-alphanumeric characters, splits into words, and normalizes to lowercase.

#### `pattern_matches`
*   **Signature**: `fn pattern_matches(normalized: &str, pattern: &str) -> bool`
*   **Description**: Verifies if target words exist inside cleaned inputs.

#### `apply_field_delta`
*   **Signature**: `fn apply_field_delta(state: &mut AffectState, field: &'static str, delta: f32, deltas: &mut Vec<AffectDelta>)`
*   **Description**: Increments target state metrics and clamps them to `[-1.0, 1.0]` (or `[0.0, 1.0]` for irritation).

---

## 4. Background Classifier Task (`classifier.rs`)

Runs after the terminal event. It evaluates complex expressions like sarcasm, updating database registries.

#### `classify_for_config`
*   **Signature**: `pub async fn classify_for_config(config: &ene_config::EneConfig, model_override: Option<&str>, max_tokens: u32, context: &ClassifierContext, timeout_secs: u64, lang: &str) -> Result<AffectProposal, CognitionError>`
*   **Description**: Resolves AI providers for classification, builds system and user prompts, compiles context, and dispatches requests.

#### `classify_failure_reason`
*   **Signature**: `pub const fn classify_failure_reason(error: &CognitionError) -> &'static str`
*   **Description**: Logs textual reasons for classification errors.

#### `proposal_json_schema`
*   **Signature**: `fn proposal_json_schema() -> serde_json::Value`
*   **Description**: Builds the schema defining classification outputs (valence, arousal, irritation, affinity, confidence, reasons).

#### `classify_with_resilient_fallback`
*   **Signature**: `async fn classify_with_resilient_fallback<F>(mut provider_factory: F, current_affect: &str, conversation: &str, timeout_secs: u64, lang: &str) -> Result<AffectProposal, ClassifierError> where F: FnMut(Option<u32>) -> Result<Box<dyn LlmProvider>, ClassifierError>`
*   **Description**: Attempts to execute requests on the primary provider, falling back to other providers on failure.

#### `classify_with_timeout`
*   **Signature**: `async fn classify_with_timeout(provider: &dyn LlmProvider, current_affect: &str, conversation: &str, timeout_secs: u64, lang: &str, transport: ClassifierTransport, json_schema: &serde_json::Value) -> Result<AffectProposal, ClassifierError>`
*   **Description**: Formats prompts and executes completion requests with timeout limits.

#### `call_provider`
*   **Signature**: `async fn call_provider(provider: &dyn LlmProvider, messages: &[LlmMessage], transport: ClassifierTransport, json_schema: &serde_json::Value) -> Result<String, ClassifierError>`
*   **Description**: Handles SSE streams or direct HTTP requests to retrieve LLM output strings.

#### `strip_markdown_fences`
*   **Signature**: `fn strip_markdown_fences(raw: &str) -> &str`
*   **Description**: Cleans markdown wrappers (e.g. ` ```json `) to isolate JSON strings.

#### `parse_proposal_json`
*   **Signature**: `fn parse_proposal_json(raw: &str) -> Result<AffectProposal, ClassifierError>`
*   **Description**: Deserializes and validates classification metrics.

#### `clamp_absolute`
*   **Signature**: `const fn clamp_absolute(v: f32, min: f32, max: f32) -> f32`
*   **Description**: Clamps value components.

---

## 5. Parameter Modification Methods (`types.rs`)

#### `with_proposal`
*   **Signature**: `pub fn with_proposal(mut self, proposal: AffectProposal) -> Self`
*   **Description**: Appends a pending classifier proposal into `TurnAffectInput`.
