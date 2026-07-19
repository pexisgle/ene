# ADR: Proactive Companion Speech

- **Status:** Accepted
- **Date:** 2026-07-18

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
| `screen_digest` | string | Internal reorganization of `screen_summary` before the speak/silence decision; empty when no screen context; never spoken verbatim; 1–4 short lines |
| `should_speak` | bool | Required; missing → `false` |
| `confidence` | f64 | Must be finite and in `[0.0, 1.0]`; out-of-range → fail-closed (`should_speak = false`); missing → `0.0` |
| `reason` | string | Internal diagnostic; never spoken verbatim; 1–3 short lines; when `screen_digest` is non-empty, must ground the decision in it |
| `topic_hint` | string | Optional hint for generation; empty if missing; 0–2 lines; must not copy `reason` or `screen_digest` verbatim |
| `urgency` | string | One of `low` / `normal` / `high`; unknown → `normal` |

**Recommended output order** (prompt + local JSON grammar): `screen_digest` → `reason` → `should_speak` → `confidence` → `topic_hint` → `urgency`. The Rust parser accepts any key order; grammar-constrained local models follow schema property order. Missing `screen_digest` normalizes to `""`.

**Generation hints** (`generation_hint_idle` / `generation_hint_with_topic`) allow up to 2–3 short lines. **Screen summaries** allow 4–6 short lines of plain text (one fact per line preferred). The vision user prompt includes the privacy-safe OS app label as a prior; summaries must still ground claims in visible UI.

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
| `screen_summary` | Short-lived **text** summary from the desktop summarizer | Desktop captures a **fresh** active-window (or primary display) frame on every observe tick (no cross-tick cache), summarizes via the **local** proactive GGUF + `mmproj` (Gemma 4), and forwards **text only** into mind. The same frame is kept **ephemerally in the runtime actor** (JPEG data URI, never mind/store). When generation uses a cloud task with `supports_vision: true`, that frame is attached to the proactive generation turn. When this source is enabled, each observe cycle drives the decision LLM immediately after vision |

Each source has an independent enable flag. When disabled, desktop must not capture that source, and mind must not include it in the decision prompt.

### Model routing

Model routing lives under `ai.tasks` in settings (see [Configuration](../configuration/settings.md#ai--provider-registry-and-task-routing)):

| Role | Config |
|---|---|
| Generation | `ai.tasks.proactive` if set, else `ai.tasks.chat` |
| Classifier (affect) | `ai.tasks.classifier` if set, else `ai.tasks.chat` |
| Decision | `ai.tasks.proactive` with `provider: "local"` resolves the named `local_models` entry → in-process GGUF; otherwise cloud decision via the chat provider |

`provider: "local"` loads a GGUF in-process via llama-cpp-2. Set `url` in `ai.local_models`; weights download into `{assets_dir}/models/gguf/` on first startup (parallel prefetch, progress logged as `[GgufDownload]`). `acceleration` is `auto` / `vulkan` / `cuda` / `cpu`. On local load failure the decision backend falls back to disabled (fail-closed) — never silently upload observation context to the cloud.

GGUF weights are **not** bundled with the app; paths are configured by the user. No external `llama-server` binary is required.

### Fail-closed summary

- Invalid config → safe defaults / clamps; feature stays off unless explicitly enabled.
- Local model missing, load failure, timeout → typed error → no speech (optional cloud fallback only if configured).
- Malformed decision JSON → `should_speak = false`.
- User `run()` during decision → discard decision; prefer the user turn.
- Shutdown cancels timer, in-flight decision, generation, and releases local llama.cpp handles.

## Related

- [Cognitive Runtime ADR](cognitive-runtime.md)
- [API v1 ADR](api-v1.md)
