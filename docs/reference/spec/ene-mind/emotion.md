# `EmotionEngine` / PAD Emotion State Specifications

This document defines the technical specifications of Ene's emotion state manager, implemented via the PAD (Pleasure-Arousal-Dominance) emotion model combined with deterministic appraisal rules and asynchronous LLM classifier loops.

---

## 1. Emotion Space Model (PAD Space)

Ene tracks emotional states using the following continuous values (stored in SQLite under `affect_states` table):

*   **Valence (Pleasure) [-1.0 ..= 1.0]**: Happiness (positive) vs Sadness (negative).
*   **Arousal [-1.0 ..= 1.0]**: Excitement (high arousal) vs Calmness (low arousal).
*   **Irritation [0.0 ..= 1.0]**: Stress and irritation buildup.
*   **Affinity [-1.0 ..= 1.0]**: Friendship and trust towards the user.
*   **Fatigue [0.0 ..= 1.0]**: Fatigue score.

---

## 2. Emotion Update Lifecycle (`update_turn`)

At the start of every turn, `before_turn` triggers `EmotionEngine::update_turn`:

```rust
pub fn update_turn(
    &self,
    config: &EmotionConfig,
    input: &mut TurnAffectInput<'_>,
) -> AffectUpdateResult
```

### 1. Temporal Decay (`apply_decay`)
*   **Decay Mechanics**: Emotional deviations decay back to the neutral baseline (0.0) over time.
*   **Decay Formula**:
    $$V_{\text{new}} = V_{\text{old}} \times e^{-\lambda t}$$
    where $\lambda = \frac{\ln(2)}{\text{decay\_half\_life\_minutes}}$. The decay is computed on `elapsed_since_update`.

### 2. Deterministic Appraisal (`apply_appraisal`)
Applies immediate changes based on raw punctuation and text patterns:
*   **Arousal Spikes**: Triggered by multiple exclamation marks ("!!"), capital letters in English, or long text sentences.
*   **Irritation Accrual**: Triggered by extremely long user prompts or specific rejection/aggressive keywords.
*   **Affinity Bumps**: Triggered by thank-you markers ("thank you", "thanks", etc.) or polite greetings.

### 3. LLM Classifier Proposal Blend (`merge_classifier_proposal`)
*   **Weighted Linear Interpolation**:
    Blends the `AffectProposal` calculated from the previous turn's LLM classifier background run:
    $$X_{\text{new}} = (X_{\text{proposal}} - X_{\text{current}}) \times w + X_{\text{current}}$$
    where $w$ is the `confidence` score (clamped between 0.0 and 1.0). Confidences below `min_confidence` are skipped.

### 4. Mood Label Classification (`compute_mood_label`)
Maps the current continuous coordinates onto a discrete label:
*   `Joyful`: Valence > 0.3, Arousal > 0.0
*   `Relaxed`: Valence > 0.2, Arousal <= 0.0
*   `Anxious`: Valence < -0.2, Arousal > 0.2
*   `Depressed`: Valence < -0.2, Arousal <= 0.2
*   `Hostile`: Irritation > 0.5
*   `Neutral`: Default fallback.

---

## 3. Asynchronous Post-Turn Classifier (`classifier.rs`)

Nuanced appraisals (like recognizing irony or sarcasm) are delegated to the LLM classifier task run in the background *after* the terminal event.

*   **Context Payload (`ClassifierContext`)**:
    -   The compact emotional snapshot at turn start.
    -   The user's input message and the assistant's resulting reply.
*   **JSON Schema**:
    Prompts the LLM to output a JSON object:
    ```json
    {
      "user_emotion": "inferred user emotion",
      "user_intent": "inferred user intent",
      "valence": -0.1,
      "arousal": 0.4,
      "irritation": 0.0,
      "affinity": -0.2,
      "recommended_expression": "sad",
      "confidence": 0.90,
      "reason": "User complained about a mistake."
    }
    ```
*   **Staging Mechanism**:
    The result is committed to the `pending_affect_proposals` table. **It is consumed at the start of the subsequent turn**, preventing slow LLM calls from introducing latency in the active chat stream.
