# `TypedMemory` / Vector Search Queries & Scoring Specifications

This document defines Ene's persistent memory kinds, statuses, scopes, hybrid search candidates collection (`MemoryStore::search`), and the final scoring mathematical model.

---

## 1. Factual Classifications & Lifecycles

### 1. Memory Kind (`MemoryKind` / Enum)
Every saved memory row is classified into exactly one kind:
*   `Episodic`: Record of past events (who said what, when).
*   `Semantic`: General factual knowledge.
*   `UserProfile`: Facts about the user (e.g. birthday, name).
*   `Relationship`: History of bonds between the companion and user.
*   `Affective`: Emotional state anchors.
*   `Commitment`: Promises and tasks made.
*   `Preference`: User likes, dislikes, and preferences.
*   `Procedure`: Instructions and how-to guides.
*   `Reflection`: Self-reflection evaluations compiled by the companion.

### 2. Memory Status (`MemoryStatus` / Enum)
Tracks memory accessibility:
*   `Active`: Relevant, retrievable memory.
*   `Faded`: Decayed, but still retrievable at lower priority.
*   `Archived`: Stored in backup. Excluded from active recall.
*   `Disputed`: Contradictory state. Marked as uncertain in system prompts.
*   `Superseded`: Overwritten by newer factual evidence.
*   `UserDeleted`: Deleted via explicit user commands.

---

## 2. Serialization & Validation Helpers (`typed_memory.rs`)

#### `as_str` (for MemoryKind, MemoryStatus, MemoryScope, MemorySource)
*   **Signature**: `pub const fn as_str(self) -> &'static str`
*   **Description**: Translates enums into database-safe string representations.

#### `from_db_str` (for MemoryKind, MemoryStatus, MemoryScope, MemorySource)
*   **Signature**: `pub(crate) fn from_db_str(s: &str) -> Self`
*   **Description**: Parses string constants loaded from SQLite back into typed enums.

#### `MemoryConfidence::new` / `MemorySalience::new`
*   **Signature**: `pub fn new(raw: f32) -> Self`
*   **Description**: Constructors that clamp raw confidence and salience float inputs to the `[0.0, 1.0]` range.

#### `MemoryConfidence::get` / `MemorySalience::get`
*   **Signature**: `pub const fn get(self) -> f32`
*   **Description**: Returns the clamped float value.

---

## 3. Forgetting Lifecycles & Transitions (`forgetting.rs`)

#### `user_restorable_statuses`
*   **Signature**: `pub const fn user_restorable_statuses() -> &'static [MemoryStatus]`
*   **Description**: Lists states that can be restored (e.g. `Faded`, `Archived`, `UserDeleted`).

#### `validate_user_restore`
*   **Signature**: `pub fn validate_user_restore(from: MemoryStatus) -> Result<(), InvalidTransition>`
*   **Description**: Validates that target memory states belong to the restorable list.

#### `validate_transition`
*   **Signature**: `pub const fn validate_transition(from: MemoryStatus, to: MemoryStatus) -> Result<(), InvalidTransition>`
*   **Description**: Enforces state machine bounds (e.g. allowing `Active` to transition to `Faded` but blocking transitions to `Superseded` without updates).

#### `emotional_impact`
*   **Signature**: `pub fn emotional_impact(affect: AffectAnnotation) -> f32`
*   **Description**: Computes emotional salience vectors.

#### `active_decay_anchor` / `faded_decay_anchor`
*   **Signature**: `pub fn active_decay_anchor(item: &MemoryItem) -> DateTime<Utc>` (same pattern for faded)
*   **Description**: Establishes base dates (`last_accessed_at`, `faded_at` or `created_at`) used in decay calculation intervals.

#### `decay_score`
*   **Signature**: `pub fn decay_score(item: &MemoryItem, now: DateTime<Utc>, half_life_days: f64) -> f32`
*   **Description**: Computes current recall scores based on decay. Pinned items retain a score of `1.0`.

#### `target_status_after_decay`
*   **Signature**: `pub fn target_status_after_decay(current: MemoryStatus, score: f32) -> Option<MemoryStatus>`
*   **Description**: Determines transitions: `Active` to `Faded` if the score drops below `0.3`, and `Faded` to `Archived` if the score drops below `0.1`.

---

## 4. Candidate Scoring & Similarity (`search.rs`)

#### `tokenize`
*   **Signature**: `pub(crate) fn tokenize(text: &str) -> HashSet<String>`
*   **Description**: Splits query text into lowercase words.

#### `document_lexical_similarity`
*   **Signature**: `pub fn document_lexical_similarity(title_a: &str, content_a: &str, title_b: &str, content_b: &str) -> f32`
*   **Description**: Measures text overlap between two documents using the Jaccard index.

#### `lexical_overlap_score`
*   **Signature**: `pub(crate) fn lexical_overlap_score(query: &str, title: &str, content: &str) -> f32`
*   **Description**: Measures word overlap between a query and a document's title and content fields.

#### `recency_score`
*   **Signature**: `pub(crate) fn recency_score(reference: DateTime<Utc>, item: &MemoryItem, half_life_days: f64) -> f32`
*   **Description**: Computes recency metrics based on the elapsed time since the record was last accessed.

#### `emotional_match_score`
*   **Signature**: `pub(crate) fn emotional_match_score(query_affect: Option<AffectAnnotation>, item_affect: AffectAnnotation) -> f32`
*   **Description**: Computes correlation scores between a query's PAD weights and the memory's emotional coordinates.

#### `relationship_score`
*   **Signature**: `pub(crate) fn relationship_score(impact: f32) -> f32`
*   **Description**: Scales relationship scores.

#### `access_boost_score`
*   **Signature**: `pub(crate) fn access_boost_score(access_count: i64) -> f32`
*   **Description**: Computes access frequency bonuses ($0.02 \times \text{access\_count}$, capped at 0.2).

#### `contradiction_penalty` / `stale_penalty`
*   **Signature**: `pub(crate) fn contradiction_penalty(status: MemoryStatus) -> f32` (same pattern for stale)
*   **Description**: Deducts score points for disputed or faded memories.

#### `is_recallable_status`
*   **Signature**: `pub(crate) const fn is_recallable_status(status: MemoryStatus) -> bool`
*   **Description**: Returns `true` if the memory status allows it to be retrieved for active recall (e.g. `Active`, `Faded`, `Disputed`).

#### `score_candidate`
*   **Signature**: `pub(crate) fn score_candidate(options: &Query<'_>, candidate: &GatheredCandidate) -> MemoryScoreBreakdown`
*   **Description**: Evaluates search candidates against the scoring formula.
