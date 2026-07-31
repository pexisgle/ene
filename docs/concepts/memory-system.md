# Long-Term Memory System & Hybrid Recall

Ene's long-term memory architecture provides persistent recall across sessions, typed memory classification, hybrid search scoring, and background memory consolidation.

---

## 1. Typed Memory Categories

Memories in `ene-store` are typed via `MemoryKind` (supporting `Episodic`, `Semantic`, `UserProfile`, `Relationship`, `Affective`, `Commitment`, `Preference`, `Procedure`, `WorldState`, and `Reflection`). The primary categories for conversation context are:

| Memory Type | Scope | Example |
|---|---|---|
| **Episodic** | Event logs & past interaction summaries | *"User discussed building a Rust game engine on Friday."* |
| **Semantic** | General facts & learned knowledge | *"The user prefers Neovim over VSCode."* |
| **Preference** | Expressed user likes, dislikes, & habits | *"Dislikes dark roast coffee."* |
| **Commitment** | Active tasks, promises, & agreements | *"Promised to check deploy logs tomorrow at 10 AM."* |

---

## 2. The Hybrid Recall Algorithm

During `before_turn`, `ene-mind` asks `ene-store` to *gather* candidates (vector, lexical, recent, and commitment lookups) and then hands them to `ene-rag` to *score* with a multi-factor **Hybrid Search Score**:

$$S = w_v \cdot S_{\text{vector}} + w_l \cdot S_{\text{lexical}} + w_r \cdot S_{\text{recency}} + w_s \cdot S_{\text{salience}}$$

Where:
- **$S_{\text{vector}}$**: Cosine similarity using `sqlite-vec` embeddings.
- **$S_{\text{lexical}}$**: Full-text search BM25 match score.
- **$S_{\text{recency}}$**: Exponential decay based on time elapsed since creation.
- **$S_{\text{salience}}$**: Importance rating assigned during memory extraction.

### Maximal Marginal Relevance (MMR) Diversification
To prevent injecting duplicate or redundant facts into the prompt packet, candidate memories pass through MMR reranking to maximize diversity.

### Access Bump Policy
When a memory is recalled, its `access_count` and `last_accessed_at` are bumped — but **only if it actually makes it into the prompt**. The budget manager may drop a recalled memory's whole section or trim it within the section when over the token budget; such a memory is recalled by search yet never seen by the model, so it is *not* bumped. The set of memories that survive packing is tracked (`PromptPacketMeta::injected_memory_ids`) and only those are bumped after composition. This prevents "ranked high in search but dropped" from reinforcing a memory (#345).

The bump's effect on ranking is also bounded: the access boost fades with the age of the most recent access (a half-life decay), so accesses from long ago stop counting and a memory cannot lock in a permanent ranking advantage.

### Contradiction Resolution (Memory Arbiter)
Before a candidate memory is persisted, the memory arbiter checks whether it contradicts an existing memory of the same kind (`Preference`, `UserProfile`, `Semantic`, `Relationship`). Two memories are treated as the same subject when the cosine similarity of their **title embeddings** reaches `mind.memory.contradiction_title_similarity_threshold` (default `0.82`) (#351), so synonymous titles ("職業" vs "仕事") collapse into one subject instead of accumulating contradictory duplicates. When no embedding provider is configured, the arbiter falls back to exact normalized-title matching.

### Self-Reflection Feedback Loop
When `mind.memory.reflection.enabled` is set, the post-turn pipeline periodically reviews persisted memory outcomes and writes `Reflection` memories summarizing successful and unsuccessful interaction strategies. These reflections **close the loop during recall**: they are loaded and applied as a scoring signal that boosts memories matching successful strategies and penalizes those matching strategies to avoid. Reflections are deliberately **excluded from the recall candidate set** (via a kind filter on the search query), so they adjust scores without ever surfacing as ordinary recall results or leaking into the LLM context. Each applied adjustment is recorded in the score breakdown's `reflection_multiplier`, keeping the explainable score consistent with its displayed total.

---

## 3. The Commitment Ledger

Commitments (promises made by Ene or requests from the user) are given special status:

- **Single Source of Truth**: Commitment entities are tracked with strict lifecycle states (`Active`, `Fulfilled`, `Cancelled`, `Expired`).
- **Ledger Verification**: Active commitments are prioritized in prompt packet assembly to prevent Ene from forgetting agreed tasks.
- **Fuzzy Title Matching (#387)**: When an embedding provider is available, the ledger matches incoming commitments against active ones by the *similarity of their title embeddings* rather than exact string equality. A rephrased promise ("資料をまとめる" vs "資料作成") therefore supersedes the existing commitment instead of being registered as a contradictory duplicate. The similarity cutoff is configurable via `mind.memory.commitment_title_similarity_threshold`; without an embedding provider the ledger falls back to exact normalized-title matching.

---

## 4. Background Consolidation & Forgetting

Memory persistence operates asynchronously to keep turn latencies low:

1. **Synchronous Turn Completion**: When a turn finishes, `EneEvent::Terminal` is emitted immediately.
2. **Deferred Memory Extraction**: A background task inspects the turn transcript, extracts candidate facts, generates embeddings, and saves them to SQLite.
3. **Forgetting & Decay**: Low-salience memories gradually decay in score and are archived or purged according to retention rules.

### Forgetting Anchor
Forgetting decay measures from the memory's last **content update** (`updated_at`), not from when it was last recalled (`last_accessed_at`). Recall recency (which ranks candidates) still uses `last_accessed_at`, but the two are deliberately separate: a memory being recalled must not reset the clock on its own forgetting. If it did, a frequently-recalled memory could never reach the fade threshold, and recall would simultaneously raise a memory's score *and* shield it from forgetting — a self-reinforcing loop (#345). Editing a memory's content refreshes `updated_at` and legitimately keeps it alive; merely recalling it does not.

---

## 5. Tool-Derived Memory Guardrails

Memories extracted from tool execution results — most notably the `Reflection`
records written when a tool call fails (`persist_failure_reflection`, on by
default) — are inherently noisy: the same tool can fail on every turn with a
*different* error message, so content-based deduplication alone never matches.
Left unchecked these records accumulate without bound. Two guardrails keep them
in check without touching unrelated memory kinds:

- **Stable-key supersede dedup.** Tool-derived candidates are keyed on their
  stable `(kind, title)` pair (e.g. `Reflection` + `tool failure:{tool_name}`)
  rather than their volatile content. When a new candidate matches an existing
  active record on that key, the arbiter *supersedes* the prior record instead
  of inserting a duplicate, so repeated failures of the same tool collapse into
  a single, always-current memory row.
- **Lightweight validity check.** Before a tool-derived candidate is persisted
  it must pass a validity gate: the tool named in its title must actually have
  been invoked during the turn, and its content must carry more than boilerplate
  failure text. Candidates that fail the gate are rejected outright.

### Failure feedback in tool selection

The recall side closes the loop: the tool-selection pipeline in `ene-rag` can
down-weight tools that have recently failed for the active character. Recent
failures are read through the `ene_core::ToolFailureSignalPort` abstraction
(implemented by `ene-store`), and a configurable penalty
(`tools.rag.use_failure_feedback` / `tools.rag.failure_penalty`) is applied to
their scores before ranking. This keeps `ene-rag` free of any persistence
dependency while still letting a consistently-failing tool sink in the ranking.
