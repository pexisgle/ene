// Memory Writer: deterministic/LLM extraction + Memory Arbiter.
//
// - `candidate` — shared types (MemoryCandidate, Locale, TurnInput)
// - `deterministic` — pattern-based extraction (no LLM required)
// - `llm` — LLM-based extraction (placeholder for #66)

pub mod candidate;
pub mod deterministic;
pub mod llm;

/// Memory Writer orchestrator — marker struct for Phase 10 (#100).
pub struct MemoryWriter;
