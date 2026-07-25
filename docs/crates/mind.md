# `ene-mind` — API Reference

> **Crate**: `ene-mind` | **Role**: Cognitive turn engine, prompt budgeting, affect, & memory recall

`ene-mind` is the pure cognitive core of Ene. It owns prompt composition, session management, identity kernel protection, PAD affect dynamics, proactive speech evaluation, performance cue arbitration, and background memory writing.

---

## Architectural Boundary Guarantee
`ene-mind` **never** imports or depends on `ene-runtime`, `ene-plugin-host`, or `ene-vrm`.

---

## Key Modules & Types

### `SessionManager`
Manages chat session state, dialogue history context, and automatic session compression/splitting:

```rust
pub struct SessionManager { /* ... */ }
```

### `PromptComposer`
Constructs sectioned `PromptPacket`s with explicit token budget allocations:

```rust
pub struct PromptComposer { /* ... */ }

impl PromptComposer {
    pub async fn compose(
        &self,
        input: &str,
        session: &Session,
        recalled: &[ScoredMemory],
    ) -> Result<PromptPacket, CognitionError>;
}
```

### `PadEmotion`
Maintains character affect state in 3D Pleasure-Arousal-Dominance space:

```rust
pub struct PadEmotion {
    pub pleasure: f32,  // [-1.0, 1.0]
    pub arousal: f32,   // [-1.0, 1.0]
    pub dominance: f32, // [-1.0, 1.0]
}

impl PadEmotion {
    pub fn update(&mut self, delta: PadDelta);
    pub fn to_performance_cue(&self) -> PerformanceCue;
}
```

### `ProactiveEngine`
Evaluates user idle time and context to trigger autonomous proactive speech:

```rust
pub struct ProactiveEngine { /* ... */ }
```

### `MemoryWriter`
Asynchronously extracts episodic & semantic facts from turn transcripts and persists them to `ene-store`.

---

## Related Links
- [Turns & Sessions](../concepts/turn-and-session.md)
- [Memory & Recall](../concepts/memory-system.md)
