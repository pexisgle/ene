//! Read-only query handles that bypass the turn-execution actor mailbox.
//!
//! Session CRUD and candidate approval only ever touch `MemoryStore` —
//! never the actor's turn-execution state (`active_turn` / `stream_handle`
//! / `turn_gate`) — so routing them through the single actor mailbox would
//! needlessly serialize read-only queries behind whatever `Run` turn
//! happens to be in flight. [`sessions::SessionQueryHandle`] and
//! [`candidates::MemoryCandidateHandle`] talk to `Arc<MemoryStore>`
//! directly instead.

/// Pending memory-candidate approval flow (list / approve / reject).
pub mod candidates;
/// Session CRUD queries (list / export / import / search / archive).
pub mod sessions;
