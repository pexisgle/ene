# Architecture Overview

ene is a modular Rust workspace centered on `ene-core` orchestration and the `ene-cognition` cognitive runtime.

## Runtime Architecture

The actor model remains the execution shell (`EneHandle` / `EneActor`), while turn intelligence is handled by cognitive components.

### Core Turn Flow

```text
User input
  -> before_turn (recall planning + affect update)
  -> compose_prompt_packet (sectioned context + budgeting)
  -> LLM streaming
  -> output arbitration (engine-managed expression)
  -> after_turn (memory write + forgetting + affect persist)
```

`ene-core` integrates this flow in the streaming lifecycle and emits runtime events for desktop/CLI consumers.

## Key Crates

- `ene-core`: actor runtime, streaming lifecycle, event bus, tool orchestration.
- `ene-cognition`: recall planner, prompt packet composer, emotion engine, output arbiter, memory writer, context compression.
- `ene-memory`: typed-memory persistence, hybrid search scoring, commitment and affect state storage.
- `ene-session`: conversation state container and compatibility split/compression hooks.
- `ene-provider`: LLM and embedding provider abstractions.
- `ene-tool-*`: sandboxed tool runtime and IPC protocol.

## Memory Model

The system uses typed memory (`episodic`, `semantic`, `preference`, `commitment`, etc.) with lifecycle statuses (`active`, `faded`, `archived`, `disputed`, `superseded`, `user_deleted`).

Hybrid recall combines:

- vector similarity
- lexical overlap
- recency decay
- salience/confidence
- affect and commitment signals

## Prompt Model

Prompt construction is sectioned (`PromptPacket`) with explicit budgets. Identity and output-contract sections are protected from dropping under budget pressure.

## Emotion and Expression

- Affect state is persisted engine-side.
- Optional LLM classification is advisory.
- Final expression is selected by Output Arbiter with hysteresis.
- Consumers receive `EneEvent::Expression` for rendering/UI.

## Applications

- `ene-cli`: REPL + debug commands for memory/affect/commitments.
- `ene-desktop`: `winit` + `wgpu` + `egui` shell with VRM rendering and cognitive debug UI.

## Reference

For full design details and terminology, see `docs/architecture/cognitive-runtime.md`.
