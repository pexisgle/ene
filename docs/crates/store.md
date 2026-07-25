# `ene-store` — API Reference

> **Crate**: `ene-store` | **Role**: Database & vector persistence layer (SQLite + SeaORM + sqlite-vec)

`ene-store` is the sole owner of database connections, SeaORM entities, SQLite schema migrations, and vector similarity search via `sqlite-vec`.

---

## Architectural Boundary Guarantee
`ene-store` **never** imports or depends on `ene-ai` or `ene-mind`.

---

## Key Structures & API

### `MemoryStore`
The central persistence interface for managing typed memories, session dialogue histories, and commitments:

```rust
pub struct MemoryStore { /* ... */ }

impl MemoryStore {
    /// Opens or creates the SQLite memory database at the target path.
    pub async fn open(db_path: impl AsRef<Path>) -> Result<Self, EneMemoryError>;

    /// Saves a newly extracted typed memory fact.
    pub async fn save_memory(&self, memory: &NewMemory) -> Result<MemoryId, EneMemoryError>;

    /// Executes multi-vector hybrid recall search (Vector + Lexical + Recency + Salience).
    pub async fn hybrid_recall(&self, query: &MemoryQuery) -> Result<Vec<ScoredMemory>, EneMemoryError>;

    /// Appends dialogue turns to an active session history.
    pub async fn append_session_turn(&self, session_id: SessionId, turn: &TurnRecord) -> Result<(), EneMemoryError>;

    /// Updates active commitment state in the commitment ledger.
    pub async fn update_commitment(&self, id: CommitmentId, status: CommitmentStatus) -> Result<(), EneMemoryError>;
}
```

---

## Related Links
- [Memory System & Hybrid Recall](../concepts/memory-system.md)
- [System Architecture](../architecture.md)
