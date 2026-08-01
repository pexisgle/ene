# `ene-runtime`

> **Crate**: `ene-runtime` | **Role**: Actor-based host facade & turn engine

`ene-runtime` is the primary entry point for applications (`ene-cli`, `ene-desktop`) embedding Ene. It owns `EneHandle`, the thread-safe facade that coordinates turn execution, prompt composition (`ene-mind`), memory storage (`ene-store`), plugin supervision (`ene-plugin-host`), and the tool DB IPC socket server.

---

## Architectural boundaries

- `EneHandle`'s public methods are non-blocking channel sends or oneshot async requests into a single-threaded actor (`handle::actor::TurnActor`); they never touch shared mutable state directly.
- Read-only session/candidate queries and screen-image vision summarization bypass the actor mailbox entirely and talk to `ene-store` / the vision model directly — they do not compete with turn-execution commands for actor throughput.
- Small per-frame state is mirrored into mailbox-free shared slots (#407): `EneHandle::card_name()`, `session_id()`, `session_started_at()`, `turn_count()`, `config()`, and `character_card()` each take one `parking_lot` lock (or an atomic) and read a slot the actor keeps in sync at the mutation point (session split, `SetCharacter`, per-turn bookkeeping, feature-settings updates) — safe to call from egui immediate mode, never queueing behind an in-flight `Run` turn. Only the large history payload stays mailbox-based, via the dedicated `EneHandle::history()`.
- Tool operations (`list` / `search` / `call` / `invalidate`) have their own handle, `EneHandle::tools()` (#406), but deliberately stay on the actor mailbox: tool calls and searches are admission-capped there (Stage 8, `EneRuntimeError::Busy`) and the registry is actor-owned state, swapped on plugin-host reconfiguration. Unlike the read-only handles, `ToolHandle` is an API-shape split, not a transport bypass.
