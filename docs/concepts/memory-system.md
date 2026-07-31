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
3. **Forgetting & Decay**: Unaccessed, low-salience memories gradually decay in score and are archived or purged according to retention rules.
