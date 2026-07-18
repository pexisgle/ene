# ADR: Proactive Companion Speech

- **Status:** Accepted
- **Date:** 2026-07-18
- **Epic:** [#103](https://github.com/pexisgle/ene/issues/103) — Proactive companion speech

## Context & Problem

Companion / AITuber experiences feel empty when the character only replies to explicit user input. Always-on high-quality generation is too expensive and noisy. Ene needs a **two-tier** path: a lightweight local model decides whether to speak; only then does the normal generation path produce an utterance that appears in chat history.

## Decision

Adopt proactive companion speech with the following fixed contracts.

### Crate responsibilities

| Crate | Responsibility |
|---|---|
| `ene-mind` | Build `ProactiveContext`, run deterministic gates, format the decision prompt, parse/normalize `ProactiveDecision`. No OS APIs, scheduler, or UI. |
| `ene-ai` | Decision model routing: `llama_cpp` (in-process llama-cpp-2 GGUF), `cloud` (OpenAI-compatible model override), or `disabled`. Generation reuses the existing chat `LlmProvider`. Explicit async shutdown of local handles. |
| `ene-runtime` | Interval scheduling, single-flight `TurnGate` integration with user turns, `TurnOrigin`, history/event emission, observation intake API, diagnostics. |
| `ene-desktop` | Privacy-aware OS observation, settings UI, chat rendering for proactive turns. |

### Dependency rules

- `ene-mind` does **not** depend on `ene-runtime`, `ene-tool-host`, or OS observation crates.
- `ene-ai` embeds llama-cpp-2 for local decision (and local embedding); it does not spawn `llama-server` or implement Candle graphs for chat/embedding. All llama-cpp inference is serialized behind a process-wide lock.
- Desktop observation stays in `ene-desktop` (or a desktop-local platform module). Raw screenshots never enter `ene-mind` / `ene-store`.

### Decision → utterance flow

```text
Timer / observation update
  -> runtime checks enabled + cooldown + user turn gate
  -> mind builds ProactiveContext (respecting source flags)
  -> lightweight decision model returns structured Decision JSON
  -> if should_speak and TurnGate free, runtime invokes normal generation
  -> TurnStarted + TextDelta / Performance / Terminal emit with TurnOrigin::Proactive
  -> assistant response only is appended to session history and conversation_logs
```

### Decision JSON contract

The decision model must return JSON only (no utterance body). Normalized fields:

| Field | Type | Notes |
|---|---|---|
| `should_speak` | bool | Required; missing → `false` |
| `confidence` | f64 | Must be finite and in `[0.0, 1.0]`; out-of-range → fail-closed (`should_speak = false`); missing → `0.0` |
| `reason` | string | Internal diagnostic; never spoken verbatim |
| `topic_hint` | string | Optional hint for generation; empty if missing |
| `urgency` | string | One of `low` / `normal` / `high`; unknown → `normal` |

Unknown fields are ignored. Parse / timeout / provider failures are fail-closed: treat as `should_speak = false` and do not start generation.

### Turn origin and history

- `TurnOrigin::{User, Proactive}` is carried on turn-scoped events and `Terminal`.
- Proactive turns **must not** insert a synthetic user message into `ConversationSession`.
- Only the assistant response is written to history / `conversation_logs` so later user turns can see it.
- `PostTurnInput` / memory writer must not treat an empty user message as a memory candidate.
- Proactive generation is routed via `ai.tasks.proactive` when set; otherwise `ai.tasks.chat` (see **Model routing** below).
- Internal companion directives are injected as **system** messages during generation; they are not stored as user history, not embedded, and not passed to memory writers.
- `generation_timeout_seconds` caps proactive generation wall time (outer timeout wins over provider defaults).
- In-flight decision tasks are aborted when a user turn starts or the actor shuts down.
- Proactive generation defaults to `allow_tools = false`.

### Suppression policy (configurable)

| Rule | Config / behavior |
|---|---|
| Feature off | `mind.proactive.enabled` default `false` |
| User / tool / permission busy | No decision and no generation while a user turn, tool call, or permission/input wait is active |
| Min idle | Suppress if last user input was less than `min_idle_seconds` ago |
| Cooldown | After a **successful** proactive utterance (`TerminalReason::Done`), suppress for `cooldown_seconds` |
| Session cap | At most `max_turns_per_session` proactive utterances per session |
| No sources | If every input source is disabled (or unavailable), skip decision |
| Confidence | Proceed to generation only when `confidence >= decision.min_confidence` |
| Failures | Decision failure, empty generation, or local model init failure never put the actor into a sticky Error state |

### Input sources

| Source | Content | Privacy |
|---|---|---|
| `conversation` | Recent `HistoryEntry` list, truncated by char budget | Session history only |
| `activity` | Optional idle hint, **app name only** (no raw window title), recent focus change | No keylogging; titles never collected in V1 |
| `screen_summary` | Short-lived **text** summary when a desktop summarizer is available | V1 desktop reports `unavailable` when enabled but no provider is bundled; raw screenshot bytes are never persisted, logged, or sent in diagnostics |

Each source has an independent enable flag. When disabled, desktop must not capture that source, and mind must not include it in the decision prompt.

### Model routing

Model routing lives under `ai.tasks` in settings (see [Configuration](../configuration/settings.md#ai--provider-registry-and-task-routing)):

| Role | Config |
|---|---|
| Generation | `ai.tasks.proactive` if set, else `ai.tasks.chat` |
| Classifier (affect) | `ai.tasks.classifier` if set, else `ai.tasks.chat` |
| Decision | Provider referenced by `ai.tasks.proactive` (or chat provider when `proactive` is `null`): `local_gguf` with non-empty `model_path` → in-process GGUF; otherwise cloud decision via the chat provider |

`local_gguf` loads a GGUF in-process via llama-cpp-2. `acceleration` is `auto` / `vulkan` / `cuda` / `cpu`. On local load failure the decision backend falls back to disabled (fail-closed) — never silently upload observation context to the cloud.

GGUF weights are **not** bundled with the app; paths are configured by the user. No external `llama-server` binary is required.

### Fail-closed summary

- Invalid config → safe defaults / clamps; feature stays off unless explicitly enabled.
- Local model missing, load failure, timeout → typed error → no speech (optional cloud fallback only if configured).
- Malformed decision JSON → `should_speak = false`.
- User `run()` during decision → discard decision; prefer the user turn.
- Shutdown cancels timer, in-flight decision, generation, and releases local llama.cpp handles.

## Consequences

- Default settings preserve existing chat behavior (proactive off).
- Desktop must own OS-specific observation and pass a normalized `ProactiveObservation` into runtime.
- CLI/desktop `EneEvent` consumers must handle `TurnStarted` and `TurnOrigin` without breaking existing match arms (additive fields / new variants handled carefully).
- Desktop settings changes push `UpdateProactiveSettings` into the actor (no restart required).
- Guide docs must document local model placement, Vulkan/RADV requirements, and privacy implications.

## Related

- Epic [#103](https://github.com/pexisgle/ene/issues/103)
- Sub-issues #162–#170
- [Cognitive Runtime ADR](cognitive-runtime.md)
- [API v2 ADR](api-v2.md)
