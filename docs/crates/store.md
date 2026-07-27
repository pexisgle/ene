# `ene-store`

> **Crate**: `ene-store` | **Role**: Database & vector persistence layer (SQLite + SeaORM + sqlite-vec)

`ene-store` is the sole owner of the SQLite/SeaORM connection, schema migrations, and vector similarity search (`sqlite-vec`) for the entire workspace. It also runs a per-tool DB IPC server (`db_server`, Unix sockets / named pipes) so stateful tool plugins can perform scoped, schema-declared CRUD without opening their own database connection.

---

## Architectural boundaries

- `ene-store` is the **sole owner** of the SQLite / SeaORM connection and schema for the entire workspace. No other crate (`ene-mind`, `ene-runtime`, tool binaries) opens its own database connection or issues raw SQL against `memory.db`; they call into `MemoryStore`, or — for plugin binaries — the IPC-based `ene-plugin-db` client backed by `ene-store`'s `db_server`.
- `ene-store` does **not** depend on `ene-runtime`, `ene-ai`, `ene-mind`, or `ene-plugin-proto`. It depends only on `ene-config` and `ene-core`, so it sits low in the dependency graph and can be called safely from any of those crates without introducing a cycle.
- `ene-store` has no LLM, embedding-provider, or prompt-assembly dependency; callers supply vectors, and the mind runtime owns summarization and prompt formatting.
- Domain vocabulary (`AffectState`, typed-memory kinds/statuses, the commitment ledger's types) is defined in `ene-core` and re-exported here unchanged. `ene-store` owns only the SeaORM entities and SQL that convert those domain types to/from DB rows, and implements `ene_core::MemoryPort` for `MemoryStore` — the trait `ene-mind`'s cognitive logic programs against instead of this concrete type.

## Design rationale

- **Why domain vocabulary moved to `ene-core`**: before that split, PAD-style affect state, typed-memory kinds, and the commitment ledger's vocabulary lived inside `ene-store`, which inverted the intended dependency direction — `ene-mind` (a "pure cognitive mind") had to depend on the concrete persistence crate just to name its own domain concepts. `ene-core` now sits below both, with no internal workspace dependencies of its own.
- **Why a per-tool DB IPC server instead of shared file access**: stateful out-of-process tool plugins (`ene-plugin-fs`'s undo ledger, `ene-plugin-utility`'s todo store) need durable state without each becoming a second SQLite writer against `memory.db`. `db_server` enforces per-tool schema declarations and prefix-based table isolation over the wire instead.
- **Why hybrid recall scoring lives here rather than in `ene-mind`**: vector, lexical, recency, and salience scoring all need direct access to the underlying indexes (`sqlite-vec`, FTS) that only `ene-store` owns; `ene-mind` consumes the ranked results through `MemoryPort` rather than reimplementing scoring against raw rows.

## API reference

Struct and method signatures are not duplicated here — they drift. Generate rustdoc for the authoritative, current API:

```sh
cargo doc -p ene-store --open
```

Start at `MemoryStore` (`store` module) and `ene_core::MemoryPort` for the trait cognitive code programs against.

---

## Related
- [Memory System & Hybrid Recall](../concepts/memory-system.md)
- [System Architecture](../architecture.md)
