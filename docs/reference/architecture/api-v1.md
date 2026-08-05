# API v1 host contract

`ene-runtime` exposes a stable, versioned contract for embedders
(`ene-cli`, `ene-desktop`, or external clients). The contract is defined in
`ene_runtime::public_api` and is deliberately small.

## Versioning

`API_VERSION = "1"`.

A version bump is required when:

- a `Public*` type (or a field on one) is removed or renamed;
- the meaning or wire shape of an existing field changes;
- a `PublicApiError` variant is removed or reshaped;
- a contract method's signature changes.

It is **not** required when:

- new `Public*` types, variants, or optional fields are added;
- internal error enums gain variants (they project into
  `PublicApiError` via `From` impls; `PublicApiError` is
  `#[non_exhaustive]`);
- host-internal `EneHandle` methods change (they are explicitly out of
  contract).

## What is in the contract

### Public types

| Type | Meaning |
|---|---|
| `PublicChatEvent` | JSON mirror of the chat bus; tagged `type` in snake_case (`turn_started`, `text_delta`, `tool_call_start`, `tool_call_result`, `permission_required`, `user_input_required`, `context_compressed`, `terminal`, `performance`, `beat_pulse`) |
| `PublicLifecycleEvent` | JSON mirror of the lifecycle bus (`status_changed`, `pending_candidates_available`, `candidate_changed`, `memory_ledger_changed`, `tool_background_completed`, `connector_changed`) |
| `PublicSessionMeta` | Session listing metadata |
| `PublicExportedMessage` | One redacted conversation message |
| `PublicPerfCue` | One performance cue (`expression`/`motion`/`lookat`/`cancel`, source `affect`/`llm`) |
| `PublicApiError` | Stable error categories: `actor_dead`, `not_found`, `storage`, `invalid`, `internal` |
| `redact_text` / `redact_tool_arguments*` | Redaction helpers |

No `ene_store`/`ene_mind`/`ene_plugin_proto` type appears in any `Public*`
field — a compile-time test enforces this.

### Contract methods

Only these `EneHandle` methods are part of the contract (their signatures
use only `Public*` types and primitives):

| Method | Purpose |
|---|---|
| `list_sessions` | List session metadata (newest first) |
| `export_session` | Export a session as a versioned, redacted JSON bundle |
| `import_session` | Import a bundle |
| `search_sessions` | Search messages |
| `archive_session` | Archive/unarchive a session |

Everything else on `EneHandle` (`run`, `subscribe`, `take_audio_stream`,
`diagnostics`, permission/undo/feature methods, the read-only handles) is
host-internal wiring and may change without a version bump.

## The three-channel event bus

```text
chat bus      EneEvent            broadcast  subscribe()
lifecycle bus LifecycleEvent      broadcast  subscribe_lifecycle()
audio channel AudioChunk          mpsc       take_audio_stream()  (single consumer)
```

Chat and lifecycle buses have JSON mirrors; the audio channel does not (it
is a heavyweight in-process streaming path).

## Error projection

Internal errors map to `PublicApiError` categories:

| Internal failure | Category |
|---|---|
| DB/backup/migration/schema problems | `storage` |
| Invalid caller input (embedding, transition, edit, format) | `invalid` |
| "not found"-shaped store errors | `not_found` |
| Anything else (incl. new future variants) | `internal` |
| Actor task no longer running | `actor_dead` (uniform across actor-control, diagnostics, vision, tool handles) |

`run`/`cancel` keep their own error types (`RunError` with `Busy`,
`CancelError` with `TurnMismatch`) because callers branch on those
variants.

## Redaction

Every event crosses a redaction boundary before serialization: tool
arguments have sensitive keys scrubbed, free-text events pass through
secret-pattern redaction (API keys, bearer tokens, PEM), and exported
sessions are redacted at the store layer.

## Embedding the contract

The canonical example is `crates/ene-runtime/examples/minimal_chat.rs`
(host bootstrap, run a turn, consume events). `EneHandle::open` performs
bootstrap; `open_from_disk` / `open_with_config` are the config-driven
helpers used by the apps.
