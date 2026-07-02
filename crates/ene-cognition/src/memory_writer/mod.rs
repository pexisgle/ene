// Memory Writer: deterministic/LLM extraction + Memory Arbiter.
//
// - `candidate` — shared types (MemoryCandidate, Locale, TurnInput)
// - `deterministic` — pattern-based extraction (no LLM required)
// - `llm` — LLM-based extraction (#66)
// - `arbiter` — validation, deduplication, contradiction resolution (#75)

pub mod arbiter;
pub mod candidate;
pub mod deterministic;
pub mod llm;

pub use arbiter::{
    AppliedDecision, ArbiterAction, ArbiterContext, ArbiterOptions, ArbiterReason,
    ArbiterReasonCode, CandidateDecision, CandidateProvenance, MemoryArbiter, SemanticMatch,
};

/// Memory Writer orchestrator — marker struct for Phase 10 (#100).
pub struct MemoryWriter;
