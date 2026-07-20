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

## 2. Hybrid Search Pipeline (`MemoryStore::search`)

To populate the conversational prompt with context, `search` collects candidates from four channels in parallel:

1.  **Vector Search (`search_typed_memories_vector`)**:
    -   Queries `memory_embeddings` using the user prompt vector via `sqlite-vec`.
    -   Evaluates `1.0 - vec_distance_cosine(embedding, query)` to gather matches.
2.  **Lexical Search (`list_lexical_typed_memory_candidates`)**:
    -   Scans titles and content columns using SQLite `LIKE` partial matches.
3.  **Active Commitments**:
    -   Queries `list_active_commitments` and retrieves memories associated with active tasks.
4.  **Recency Fallback**:
    -   If matches are scarce, it extracts recently accessed or modified records using `list_recallable_typed_memories` up to `recent_fallback_limit`.

---

## 3. Hybrid Scoring Model

Collected candidates are evaluated via `score_candidate` (`search.rs`), sorted, and capped.

### Hybrid Score Formula (`MemoryScoreBreakdown`)

$$\text{Total Score} = W_v S_v + W_l S_l + W_r S_r + W_s S_s + W_c S_c + W_e S_e + W_{\text{affinity}} S_{\text{affinity}} + B_a + B_{\text{cmt}} - P_{\text{dispute}} - P_{\text{stale}}$$

*   **Vector Similarity ($S_v$)**: Raw cosine similarity value.
*   **Lexical Overlap ($S_l$)**: Jaccard index based on shared words in query and memory content.
*   **Recency Decay ($S_r$)**:
    $$S_r = e^{-\lambda t}$$
    where $t$ is the elapsed time in days and $\lambda$ is computed from the default half-life.
*   **Salience Score ($S_s$)**: LLM-assigned significance value at write time.
*   **Confidence Score ($S_c$)**: LLM-assigned certainty score.
*   **Emotional Match ($S_e$)**: Inverse Euclidean distance between the memory's PAD annotation and the companion's current mood.
*   **Affinity Match ($S_{\text{affinity}}$)**: Correlation with the companion's affinity level.
*   **Access Boost ($B_a$)**: Bumps score for frequently accessed items ($0.02 \times \text{access\_count}$, capped at 0.2).
*   **Commitment Boost ($B_{\text{cmt}}$)**: Bumps score if the memory is linked to an active promise.
*   **Disputed Penalty ($P_{\text{dispute}}$)**: Subtracts points if status is `Disputed`.
*   **Stale Penalty ($P_{\text{stale}}$)**: Subtracts points if status is `Faded`.

Sorted list items within the `limit` threshold are returned as `ScoredMemory` objects.
