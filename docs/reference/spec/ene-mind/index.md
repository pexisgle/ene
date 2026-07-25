# `ene-mind` Crate Role & Modules Overview

The `ene-mind` crate implements the core "Cognitive Mind" runtime of the Ene AI mascot companion. It plans context assembly (prompt packing), queries episodic and semantic memories, appraises emotions, resolves avatar expressions, and consolidating memory facts from conversation history.

---

## 1. Dependencies and Boundaries

### Physical Dependencies (`Cargo.toml`)
- **External Dependencies**: `tokio`, `serde`, `serde_json`, `chrono`, `tracing`, `async-trait`, `regex`, `parking_lot`
- **Workspace Dependencies**: `ene-ai`, `ene-config`, `ene-store`
- **Dependency Isolation**: `ene-mind` must not depend on `ene-runtime` or `ene-plugin-host`. To prevent circular references and preserve domains, it stays unaware of actor-thread loops or subprocess spawning details, exposing only a deterministic state machine interface.

### Store Isolation
- **Database Access**: `ene-mind` calls the persistence store (`ene-store`'s `MemoryStore`) only via its public API methods. It never issues raw SQL queries or references `sea-orm` structures directly.

---

## 2. Module Directory

```text
ene-mind/src/
├── lib.rs              # Crate root. Re-exports key APIs
├── config.rs           # Cognitive configuration fields (MindConfig)
├── engine.rs           # Orchestrator facade CognitionEngine
├── error.rs            # Cognitive pipeline error enum (CognitionError)
├── lifecycle.rs        # Turn-scoped DTOs (PreTurnOutput, TurnContext)
├── summarizer.rs       # Session boundary summarization utilities
├── character/          # Character lorebook & compile engines
├── commitments/        # Task tracker (CommitmentLedger)
├── context/            # Token budget & memory compression manager
├── emotion/            # PAD emotion engine & LLM appraisal
├── memory_journal.rs   # Diagnositc list query facade
├── memory_writer/      # Memory consolidation (MemoryArbiter) & decay
├── output/             # Visual expression arbiter (OutputArbiter)
├── pre_turn/           # Turn intent classifier
├── proactive/          # Proactive speech supervisor & vision summaries
├── prompt_packet/      # Prompt layout packer
├── recall/             # Memory recall planning & hybrid retrieval
└── session/            # Character card & resolved expressions
```

---

## 3. Specification Module Links

Detailed technical specifications:

*   [CognitionEngine & Turn Lifecycles](engine.md)
*   [RecallPlanner & Hybrid Recall Queries](recall.md)
*   [MemoryArbiter / Long-Term Facts Extraction](memory_writer.md)
*   [EmotionEngine / Appraisal & PAD Calculation](emotion.md)
*   [ContextManager / Token Budget & Splitting](context.md)
*   [ConversationSession / Character Card CBS](session.md)
*   [Proactive Speech / Proactive Decision Loop](proactive.md)
