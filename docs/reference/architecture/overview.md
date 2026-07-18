# Architecture Overview

ene is a modular Rust workspace centered on the API v2 host contract (`ene-runtime`) and the `ene-mind` cognitive turn pipeline.

## Runtime Architecture

The actor model remains the execution shell (`EneHandle` / actor), while turn intelligence is owned by `ene-mind`.

### Core Turn Flow

```text
User input
  -> before_turn (recall planning + affect update)
  -> compose_prompt_packet (sectioned context + budgeting)
  -> LLM streaming
  -> output arbitration (Performance cues)
  -> after_turn (memory write + forgetting + affect persist)
  -> Terminal (chat event after after_turn completes)
```

`ene-runtime` integrates this flow and emits a **minimal** chat event bus; diagnostics are separate.

## Target Crate Map (API v2)

| Crate | Role |
|---|---|
| `ene-runtime` | Ready `EneHandle::open`, `TurnId`, single-flight Busy, chat events, diagnostics facade |
| `ene-mind` | Identity, typed memory policy, affect, Performance arbitration, compression, session state |
| `ene-store` | SQLite-vec persistence only (`store.enabled` / `store.db_path`) |
| `ene-ai` | LLM + batch-only embedding providers |
| `ene-tool` / `ene-tool-host` | Wire/host tool ABI and process orchestration |
| `ene-config` | Settings, character cards, paths |
| `ene-vrm` | VRM rendering (no mind/runtime dependency) |

See [API v2](api-v2.md) for locked decisions and the dependency graph.

## Memory Model

Typed memory (`episodic`, `semantic`, `preference`, `commitment`, …) with lifecycle statuses. The commitment ledger is the sole source of truth for commitments. Hybrid recall (vector + lexical + recency + salience) is planned and executed by **mind**; **store** accepts text / optional precomputed vectors / filters only.

## Prompt Model

Prompt construction is sectioned (`PromptPacket`) with explicit budgets. Identity and output-contract sections are protected under budget pressure.

## Emotion and Performance

- Affect state is persisted engine-side.
- Final presentation cues are emitted as `EneEvent::Performance` (not standalone `SpecialToken` / `Expression`).
- `PerformanceCue` is owned by `ene-mind`; desktop maps cues to VRM playback without importing mind types into `ene-vrm`.

## Applications

- `ene-cli`: `ConfigStore::try_load` → card → `EneHandle::open`; REPL + diagnostics commands.
- `ene-desktop`: soft config load when needed → `open`; VRM + Performance consumption.

## Reference

- [API v2 ADR](api-v2.md)
- [Cognitive Runtime ADR](cognitive-runtime.md)
- [Avatar Performance ADR](avatar-performance.md)
- [Proactive Companion Speech ADR](proactive-speech.md)
