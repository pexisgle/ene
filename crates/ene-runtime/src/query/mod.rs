//! Session CRUD and pending-candidate **reads** only ever touch
//! `MemoryStore` — never the actor's turn-execution state (`active_turn` /
//! `stream_handle` / `turn_gate`) — so routing them through the single actor
//! mailbox would needlessly serialize read-only queries behind whatever `Run`
//! turn happens to be in flight. [`sessions::SessionQueryHandle`] and
//! [`candidates::MemoryCandidateHandle`] (and the ledger's
//! [`ledger::MemoryLedgerHandle`]) talk to `Arc<MemoryStore>` directly for
//! reads; mutations (`ResolveCandidate` / `EditCandidate`, `EditMemory` /
//! `SetMemorySalience`) go through the actor mailbox so they serialize with
//! turn execution and emit `LifecycleEvent::CandidateChanged` /
//! `MemoryLedgerChanged` audit events.

/// Pending memory-candidate approval flow (list / approve / reject).
pub mod candidates;
/// Interactive memory/commitment ledger (list / inspect / edit / salience).
pub mod ledger;
/// Session CRUD queries (list / export / import / search / archive).
pub mod sessions;
