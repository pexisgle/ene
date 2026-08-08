# Crate interfaces — overview

This section maps every crate's **public interface** — the modules, types,
traits, and functions each crate exposes — so refactoring work can see at a
glance what a change touches. Rustdoc (`cargo doc -p <crate> --open`) remains
authoritative for exact signatures; these pages name the surface and the
reasons it exists.

## The layers

```text
apps            ene-desktop, ene-cli
host facade     ene-runtime
cognitive       ene-mind
persistence     ene-store ── (sole DB owner)
vocabulary      ene-core (types + port traits)
policy          ene-rag, ene-util
provider layer  ene-ai, ene-infer, ene-voice
plugin stack    ene-plugin-proto, ene-plugin, ene-plugin-macros,
                ene-plugin-host, ene-plugin-db
external        ene-connector, ene-vrm
```

## Kinds of interfaces

| Kind | Where | Refactoring impact |
|---|---|---|
| **Host contract (API v1)** | `ene-runtime::public_api` | Stable by policy; changing it requires a version bump (see [API v1](../architecture/api-v1.md)) |
| **Port traits** | `ene-core` (`MemoryPort`, `EmbeddingStorePort`, `WorkspaceDocumentPort`, `ToolFailureSignalPort`) | The seams between mind/runtime and store; changing a port method ripples through both sides |
| **Wire ABI** | `ene-plugin-proto`, `ene-plugin-db` messages | Additive changes only; never break frames or variants |
| **Authoring facade** | `ene-plugin` (traits, `prelude`, `run_plugin_server`) | The contract every plugin binary compiles against |
| **Provider traits** | `ene-ai` (`LlmProvider`, `EmbeddingProvider`, `TtsProvider`, `SttProvider`, `VadEngine`, `ProviderHost`) | Implemented by the plugin host and provider plugins |
| **Local-model framework** | `ene-infer` (`LocalModel`, `EngineHandle`) | Implemented by local engine providers (llama.cpp, whisper, ONNX) |
| **Config schema** | `ene-config` (`define_config!`) | Adding a key is additive; removing/renaming a key needs a migration |
| **Pure policy/utility** | `ene-rag`, `ene-util` | No I/O; safe to change behind their pure function signatures |

## Dependency rules that protect the interfaces

- `ene-core` depends on nothing internal — both `ene-store` and `ene-mind`
  can use it without depending on each other.
- `ene-mind` never depends on `ene-runtime`, `ene-plugin-host`, or
  `ene-store` (production); it reaches persistence only through
  `MemoryPort`.
- `ene-store` never depends on `ene-ai` or `ene-mind`.
- `ene-rag` depends on `ene-core` only for its scoring core; the tool
  pipeline is feature-gated (`tool`) and pulls `ene-ai`.
- `ene-connector` deliberately does not depend on `ene-config` or
  `ene-plugin-proto`, so plugin binaries can use its credential types
  without dragging in the config/protocol stack.
- `ene-plugin-proto` is wire ABI only — no business logic, ever.
- `ene-vrm` is standalone (no mind/runtime/store types).

## Refactoring rules of thumb

1. **Follow the edges.** Moving a type across a dependency edge changes
   every consumer; prefer changing inside the owning crate first.
2. **Ports are the seams.** `ene-core`'s traits exist so the persistence
   implementation can be swapped (test doubles, new DBs). Extend the trait,
   not the call sites.
3. **Additive wire changes.** New `#[serde(default)]` fields and new enum
   variants are safe; renames and removals are breaking.
4. **Compile-time contracts are load-bearing.** Tests like
   `public_dto_fields_are_primitive_only` and the capability-grammar
   validation exist to catch interface leaks; keep them.
5. **`doc(hidden)` is a statement.** Items marked `#[doc(hidden)]`
   (e.g. `ene-runtime::streaming`, `message_builder`, some VRM loaders) are
   intentionally not part of the supported surface — refactor them freely,
   but do not widen their use.

## Pages

| Crate | Page |
|---|---|
| `ene-core` | [ene-core](ene-core.md) |
| `ene-config` | [ene-config](ene-config.md) |
| `ene-card` | [ene-card](ene-card.md) |
| `ene-mind` | [ene-mind](ene-mind.md) |
| `ene-store` | [ene-store](ene-store.md) |
| `ene-ai` | [ene-ai](ene-ai.md) |
| `ene-infer` | [ene-infer](ene-infer.md) |
| `ene-rag` | [ene-rag](ene-rag.md) |
| `ene-connector` | [ene-connector](ene-connector.md) |
| `ene-plugin-proto` | [ene-plugin-proto](ene-plugin-proto.md) |
| `ene-plugin` | [ene-plugin](ene-plugin.md) |
| `ene-plugin-macros` | [ene-plugin-macros](ene-plugin-macros.md) |
| `ene-plugin-host` | [ene-plugin-host](ene-plugin-host.md) |
| `ene-plugin-db` | [ene-plugin-db](ene-plugin-db.md) |
| `ene-runtime` | [ene-runtime](ene-runtime.md) |
| `ene-voice` | [ene-voice](ene-voice.md) |
| `ene-vrm` | [ene-vrm](ene-vrm.md) |
| `ene-util` | [ene-util](ene-util.md) |
