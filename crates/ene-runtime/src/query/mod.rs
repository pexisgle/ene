//! Read-only query handles that bypass the turn-execution actor mailbox (#271).
//!
//! `ListSessions` / `ExportSession` / `ImportSession` / `SearchSessions` /
//! `ArchiveSession` / `ListPendingCandidates` / `ApproveCandidate` /
//! `RejectCandidate` used to be `EneCommand` variants routed through the
//! single actor mailbox even though they only ever touched `MemoryStore` —
//! never the actor's turn-execution state (`active_turn` / `stream_handle` /
//! `turn_gate`). That needlessly serialized read-only queries behind
//! whatever `Run` turn happened to be in flight. [`sessions::SessionQueryHandle`]
//! and [`candidates::MemoryCandidateHandle`] talk to `Arc<MemoryStore>`
//! directly instead.

/// Pending memory-candidate approval flow (list / approve / reject).
pub mod candidates;
/// Session CRUD queries (list / export / import / search / archive).
pub mod sessions;
