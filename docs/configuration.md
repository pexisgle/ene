# Settings & Configuration Reference

Ene uses a layered configuration system powered by `figment`. Configuration settings are loaded in strict order of priority:

$$\text{Defaults} \longrightarrow \text{JSON Configuration File} \longrightarrow \text{Environment Variables (\texttt{ENE\_*})}$$

---

## 1. Environment Variable Precedence

Environment variables override any settings specified in default structs or JSON config files. Environment variables use the `ENE_` prefix, with double underscores (`__`) separating nested section keys:

```bash
# Example: Override default LLM chat model
export ENE_AI__TASKS__CHAT__MODEL="gpt-4o"

# Example: Specify SQLite database file path
export ENE_STORE__DB_PATH="/path/to/custom_memory.db"

# Example: Configure proactive speech interval (seconds)
export ENE_MIND__PROACTIVE__INTERVAL_SECONDS="300"
```

Environment-variable overrides are **transient**: they apply at runtime for the current process only and are never written back to `settings.json`. Saving the configuration persists only the JSON-layer values, so removing an `ENE_*` variable restores the underlying JSON/default value on the next launch (#326).

---

## 2. Configuration Sections

Public config sections are declared at their owning crates using `define_config!`.

### `ai.*` — LLM, Embeddings, & Voice Pipeline Settings

Contains provider definitions, task routing, retry/fallback rules, and voice (STT/TTS/VAD) settings:

```json
{
  "ai": {
    "providers": {
      "openai": {
        "kind": "openai",
        "api_key": "sk-...",
        "base_url": "https://api.openai.com/v1"
      }
    },
    "tasks": {
      "chat": {
        "provider": "openai",
        "model": "gpt-4o-mini"
      },
      "embedding": {
        "provider": "openai",
        "model": "text-embedding-3-small"
      }
    },
    "stt": { "enabled": true },
    "tts": { "enabled": true },
    "vad": { "enabled": true }
  }
}
```

Provider backends ship as plugins: the OpenAI-compatible backend is the
`openai` provider plugin (`plugins/provider/openai`, kind `"openai"`),
included in the default `plugins.list` with `OPENAI_API_KEY` and
`OPENAI_BASE_URL` passed through to its process. The legacy kind value
`"openai_compatible"` is still accepted as an alias, and per-provider
`base_url` / `api_key` are forwarded to the plugin per request, so existing
OpenAI-compatible configurations (OpenRouter, local servers, …) keep working
unchanged. The `openai` plugin is also the embedding backend: point
`tasks.embedding` at an `"openai"`-kind provider for cloud embeddings. With
the plugin system disabled (`plugins.enabled = false`) no cloud provider is
available. Local GGUF embeddings use `tasks.embedding.provider = "local"`
with a model key from `ai.local_models` that declares its real
`dimensions` (see the llama-cpp profile section below).

Each provider entry may set an optional `context_window` (integer, tokens) to
cap the context window the backend advertises (#364). The effective window is
`min(advertised, context_window)`, so an override can only *shrink* a model's
stated limit, never exceed it; omit it to defer entirely to the provider. A plugin provider advertises its window through `LlmProviderSpec.context_window`,
and a local model reports `LocalModelDef.context_size`. From that effective
window the runtime reserves the task's `max_tokens` (`tasks.<task>.max_tokens`)
as headroom for the model's reply, plus a safety margin that absorbs
token-estimation error, and budgets the prompt against what remains:

```
available = min(model_window, context_window)
          − response_reserve    // tasks.<task>.max_tokens
          − safety_margin       // estimation error; ~0 once usage is measured (#365)
```

```json
{
  "ai": {
    "providers": {
      "openai": {
        "kind": "openai",
        "api_key": "sk-...",
        "base_url": "https://api.openai.com/v1",
        "context_window": 32000
      }
    }
  }
}
```

Local models default `context_size` to 16,384 tokens (#366), calibrated to hold
the system's own default prompt budget (`mind.context.max_prompt_tokens` =
12,000) plus the model's reply. The previous default of 2,048 was sized for
small decision tasks and silently dropped most prompt sections once a local
model carried the main conversation. 16K is chosen over 32K to keep the
llama.cpp KV cache realistic (~2.3 GB vs ~4.6 GB for a Gemma-3-4B-class model,
on top of the weights); a model used only for decision workloads can lower
`context_size` explicitly.

Each `ai.local_models.<name>` entry's model path/settings (`url`,
`quantization`, `model_path`, `gpu_layers`, `context_size`, `dimensions`) are
mirrored into the `plugins.list.llama-cpp.profiles.<name>` blob consumed by
the local GGUF provider plugin (`ene-plugin-llama-cpp`); the `local_models`
keys themselves remain here as routing and context-budget information
(`context_size` is read at resolve time, `dimensions` is the host's
store-schema value for local embedding). The mirror is a one-way copy made by
the v2→v3 settings migration — editing a profile does not rewrite
`local_models`.

At startup the runtime validates each generative task's window (`chat`, plus
`proactive` when configured) against what it needs — the prompt budget plus the
response reserve (`tasks.<task>.max_tokens`) — and logs a warning when the
configured window is too small, since prompt sections would otherwise be
dropped every turn without any visible signal. Cloud tasks without an explicit
`context_window` override are validated at runtime instead, once the provider
reports its real window.

#### Token usage accounting (#365)

Every completion carries an optional token-usage record — `prompt_tokens`,
`completion_tokens`, and `total_tokens` — through all three provider layers
(`ene-ai`'s in-process types, the plugin IPC, and the streaming chunk). How it
is filled depends on the backend:

- **Providers that report usage** (OpenAI-compatible, Anthropic) populate it
  directly from the API response. For streaming, usage arrives on the
  **final** chunk only; intermediate chunks leave it empty.
- **Local models** (llama.cpp) count tokens themselves — the exact prompt
  length fed into the context and the number of tokens sampled — so they
  report real usage for both one-shot and streaming completions.
- **Providers that report nothing** fall back to a coarse character-based
  estimate (roughly one token per three characters), which over-counts English
  and under-counts Japanese less than a naive four-chars-per-token rule.

Because a measured count has no estimation error, the `safety_margin` above can
be driven toward zero once usage is available; the estimate keeps a conservative
margin only while a backend reports nothing. There is no configuration for this
behavior — it is automatic per provider.

### `store.*` — Database & Memory Persistence

Controls SQLite database persistence, integrity checks, and backup retention (#239):

```json
{
  "store": {
    "enabled": true,
    "backup_on_migrate": true,
    "max_backups": 5,
    "integrity_check_on_open": false
  }
}
```

### `mind.*` — Cognitive Engine & Emotion Parameters

Configures context-window packing, hybrid memory recall, emotion decay, character compilation, and proactive speech policy (#103):

```json
{
  "mind": {
    "context": {
      "max_prompt_tokens": 4096
    },
    "emotion": {
      "enabled": true,
      "decay_half_life_minutes": 30.0
    },
    "proactive": {
      "enabled": true,
      "interval_seconds": 600,
      "fatigue_suppression_threshold": 0.7,
      "confirmation_enabled": false,
      "sources": {
        "window_title_level": "app_only"
      }
    },
    "memory_limits": {
      "commitment_active_match_limit": 4096
    }
  }
}
```

Prompt packing no longer allocates per-section token budgets (#370). Instead it
fills the model's effective context window (#364) in priority order: required
sections (identity kernel, output contract, user input) are always kept, and
when the prompt overflows the window the lowest-priority droppable sections are
shed first. `mind.context.max_prompt_tokens` is an optional operator cap that
shrinks the window as `min(advertised, max_prompt_tokens)`; omit it (the
default) to let the prompt auto-follow the model's advertised context size.

Expression markers from the chat model are canonical: when a turn emits an
expression proposal it wins over affect-to-expression mapping, which remains
the fallback when no marker arrives. Hysteresis applies to every source so
rapid mid-turn markers cannot flicker the face; speech-timed expression changes
are handled separately from this resolve path.

The proactive activity observer captures the focused application to inform spontaneous
speech. `mind.proactive.sources.window_title_level` controls how much of the focused
window's title it reads (#378). It defaults to `app_only` (the app name only — the
historical behaviour), because window titles routinely contain private data: document and
file names (which can embed customer or project names), page URLs, chat contact names, and
email subjects. This text is fed to the proactive-speech decision model and, when a cloud
provider is configured, **leaves the local machine**. The levels are:

| Level | Captured |
|---|---|
| `app_only` | App name only (default; the title is never read) |
| `redacted_title` | App name + window title with filesystem paths, email addresses, URLs, and number sequences stripped (standalone document names such as `report.xlsx` are preserved) |
| `full_title` | App name + the raw window title |

Choose `full_title` only with a local model; with a cloud provider the raw title is sent
off-machine.

### Screen summary pipeline (ROI, diff gate, OCR)

`mind.proactive.sources.screen_summary` (default `false`) opts in to a short text
summary of the focused window (or primary display when Ene itself is focused).
The pipeline runs entirely in-process on the desktop:

1. **Capture + ROI composite**: the frame is captured at full resolution, shown
   to the vision model as a 50%-scale overview with a 512×512 **100%-scale crop
   around the cursor** composited beside it (separated by a dark bar). The crop
   anchor snaps to a 64px grid so small pointer moves keep the crop stable. The
   crop is only produced when both the cursor position and the captured
   surface's global geometry are known. X11 (window/monitor via xcap) is fully
   supported. On Wayland the crop is best-effort: KWin/Hyprland expose window
   geometry via `active_win_pos_rs`, but the pointer still comes from
   `device_query` over XWayland — it freezes over native Wayland surfaces, so
   the crop can lag — and on HiDPI the geometry is logical while the capture is
   physical, shifting the crop by the scale factor. GNOME Wayland exposes no
   geometry at all, so the overview is used without a crop. The crop follows
   the **pointer**, not the text caret — tracking the caret would require an
   accessibility API (AT-SPI) and is a follow-up.
2. **Diff gate**: each tick computes 64×64 grayscale fingerprints of the
   overview and the ROI. When the active app label matches, fewer than 48
   overview cells moved by ≥ 6/255, and the ROI fingerprint moved in fewer
   than 12 cells (a blink tolerance — the ROI sits at the pointer, exactly
   where a text caret blinks), the cached summary is reused and the local
   vision model is **not** invoked. Re-inference is forced on window switches,
   scrolling, word-level edits in the ROI, and any surface resize. Hits are
   observable in structured logs under `event="screen_diff_gate"` with
   `cached=true`.
3. **OCR / text hints**: the pipeline ships no OCR engine (Tesseract is not
   available in the Nix flake or the CI image, and a pure-Rust OCR would be a
   heavy new dependency). Instead, a lightweight window-title/class heuristic
   flags code editors and terminals so the model is told to prefer quoting
   visible code/errors, and a capture-time text-hint hook exists for a future
   local OCR backend (any extracted text would ride along to the vision prompt).
   The 100%-scale ROI is the primary mechanism for reading fine text today.

All processing is local: raw frames never leave the desktop process (they are
dropped after summarization), no new network path is involved, and the existing
`sources.*` privacy settings fully govern this feature. On very large displays
(5K+), the overview is scaled to fit the vision model's pixel budget — the ROI
keeps its 100% scale.

`mind.proactive.fatigue_suppression_threshold` (0.0–1.0, default `0.7`) suppresses
proactive decisions while the character's affect fatigue is at or above the threshold —
a tired character does not speak unprompted. The default matches the `"tired"` mood
label boundary (`compute_mood_label`), so the gate and the character's visible mood stay
consistent. Set it to `1.0` to disable the gate and let the decision model weigh fatigue
alone. The full affect state (all eight dimensions plus the mood label) is always passed
to the decision model regardless of this threshold.

`mind.proactive.confirmation_enabled` (default `false`) makes the main generation
model confirm the decision inside the same generation call — no extra round trip.
The generation prompt instructs the model that it may decline by emitting
`<|silent|>` as the very first token; when that token arrives before any visible
text, the runtime cancels the stream immediately and nothing is displayed or
spoken. Confirmation only raises precision: it can catch a false "speak" from the
decision model, but it cannot recover opportunities the decision model already
rejected. When confirmation is enabled, the decision threshold
(`mind.proactive.decision.min_confidence`, currently fixed at 0.55 during the
staged rollout) is therefore **automatically lowered by 0.15** — the cheap
decision model becomes a recall-first stage that lets borderline candidates
through, and the main model is the precision stage that rejects them. The
decision/main-model agreement (accepted vs. declined among decisions that reached
generation) is recorded in structured logs under `event="confirmation"`; empty
responses (no visible text) are logged with `confirmation=empty` and excluded
from the rate. Early cancellation applies to token-streaming providers; the
non-streaming local adapter buffers the full completion before its first chunk,
so a refusal there discards a completed generation rather than saving tokens.

### Quiet hours and manual pause

`mind.proactive.quiet_hours` suppresses proactive speech on a schedule. It is
disabled by default. Fragment of `mind.proactive`:

```json
"quiet_hours": {
  "enabled": true,
  "timezone": "Asia/Tokyo",
  "days": {
    "monday": true, "tuesday": true, "wednesday": true,
    "thursday": true, "friday": true, "saturday": false, "sunday": false
  },
  "start": { "hour": 22, "minute": 0 },
  "end": { "hour": 7, "minute": 0 },
  "suppress": { "notifications": true, "decisions": true, "tts": true },
  "policy": "discard"
}
```

- `timezone` is an IANA name (`Asia/Tokyo`, `America/New_York`); empty uses
  the system local timezone. DST transitions are resolved by converting the
  UTC instant to local wall time, so a fall-back repeated hour counts as
  inside the window for both occurrences and a spring-forward skipped hour
  never does.
- `days` selects weekdays. `start` is inclusive and `end` exclusive; `end`
  earlier than `start` wraps across midnight, so the window covers the start
  day's evening and the following morning, and the start day's weekday must
  be enabled. Equal start/end is an empty window.
- `suppress` picks the output channels: `decisions` stops the whole
  decision/generation pipeline at the deterministic gate (no LLM call),
  `notifications` suppresses the proactive turn's status announcement, and
  `tts` keeps the generated text but drops TTS audio for proactive turns.
- `policy` decides what happens to speech blocked by `decisions`: `discard`
  logs and drops it, `queue` delivers one catch-up utterance per missed
  moment after the window ends, and `summary` delivers a single aggregated
  catch-up line. Queue/summary only apply while `decisions` is suppressed.
  The catch-up queue is bounded (oldest moments drop first) and session
  scoped; a moment is recorded only when the deterministic warrant gates
  (idle, cooldown, session limit, sources, fatigue) would have passed, and a
  user turn clears the queue (the user is back at the desk). Catch-up items
  carry the local date and time only — never screen data.
- Background observation (activity, screen summaries) continues during quiet
  hours, governed by the existing privacy settings (`sources.*`); quiet
  hours only gate speech output. Suppression is recorded in structured logs
  under `event="quiet_hours_suppression"` with policy and decision metadata
  only — never screen images.

`mind.proactive.paused` (default `false`) is a manual pause that outranks
quiet hours and every other gate. While paused, no proactive speech happens,
any pending catch-up delivery is discarded, and the desktop settings screen
shows the pause state explicitly.

### Pending-candidate confirmation

`mind.proactive.pending_confirmation` (disabled by default) lets old
unconfirmed memory candidates be confirmed through proactive speech. Deferred
candidates that topic-near recall never surfaces (their topics never come up)
would otherwise sit in the queue forever; when this trigger is enabled, the
proactive pipeline selects the oldest candidate that is still pending, is at
least `min_age_days` old (default `3`), and carries at least `min_confidence`
(default `0.7`, clamped to `0.0..=1.0`):

```json
"pending_confirmation": {
  "enabled": false,
  "min_age_days": 3,
  "min_confidence": 0.7,
  "reask_after_days": 7
}
```

- Only weak-contradiction deferrals are eligible. Approval-mode rows
  (`approval_parked`) stay review-queue-only and are never asked about —
  an unapproved candidate must not surface as hearsay in conversation,
  mirroring the recall exclusion.
- At most one question is in flight. A selected candidate flows through the
  normal decision pipeline: every deterministic gate (manual pause, quiet
  hours, idle, cooldown, session limit, fatigue) applies unchanged, and the
  decision model judges whether now is a good moment to interrupt.
- The generation prompt asks a short, natural confirmation question; the
  candidate is presented as hearsay, never as a fact, and never with internal
  labels. With `confirmation_enabled` the model may still decline via
  `<|silent|>`.
- The user's reply is classified (approved / rejected / unclear) by the
  proactive decision model. `approved` persists the candidate through the
  approval APIs, `rejected` discards it, and `unclear` or any failure leaves
  it pending for a later attempt. Resolutions invalidate the recall cache and
  emit the same `CandidateChanged` lifecycle event as the manual review queue.
- A candidate is not selected again within `reask_after_days` (default `7`)
  of a delivered question, so an unclear reply or a failed classification
  cannot re-arm the same question on the next tick and nag the user. `0`
  disables the backoff.
- The asked marker is session-scoped and not persisted: a restart simply
  re-selects the candidate on a later tick.

Proactive decisions also consult stored memory. `mind.proactive.sources.memory` (default
`true`) feeds the user's `Preference` / `UserProfile` memories — "don't talk while I work",
"quiet at night" — into the decision context as `user_instructions`. These are injected
deterministically (newest first, up to `mind.proactive.max_memory_notes`, default 12; the
cap is fixed at 12 during the staged rollout, so the field is not user-configurable yet)
and
never pass through recall score competition, so a suppression condition cannot be dropped
by a low score; the decision model is instructed to honor a matching standing rule with
`should_speak=false`. The same setting enables topic recall during generation: the
decision's `topic_hint` becomes a lexical-only search query (no embedding provider is
involved), so the companion can refer to what it remembers about the topic. Setting
`sources.memory` to `false` restores the memory-free behaviour for cost/latency-sensitive
setups.

`redacted_title` filters the title field by field. It splits on whitespace and on the
punctuation window titles are built from (`_ - | 、 ・ 【】 「」 ｜ ：` …), so titles that
carry no spaces — the norm in Japanese and Chinese — are still filtered per field rather
than passed through as one unrecognizable blob. It deliberately does **not** split on `.`,
`/`, or ASCII `:`, since those hold paths, URLs, and file extensions together and the
detectors need them intact. A field with no separator around it (a single run of prose
containing a name, say) is still kept, so `redacted_title` reduces exposure rather than
eliminating it; use `app_only` when the title must never leave the machine.

Memory recall uses a hybrid score `(relevance × quality + commitment_boost) × penalty`
(see `crates/ene-rag/src/scoring.rs`). A fresh, strongly relevant memory scores
near `1.0`; recent/lexical-only candidates land around `0.1–0.5`; unrelated
noise scores `0.0`. `recall_min_score` (default `0.10`) filters the final
ranking, `recall_similarity_threshold` (default `0.35`) gates the vector-gather
step, and `commitment_boost` (default `0.25`) lets active promises surface even
with zero query relevance. `access_boost_half_life_days` (default `14.0`, matching
`ene_rag::ACCESS_BOOST_HALF_LIFE_DAYS`) controls how quickly prior-access boosts
fade in the quality factor — independent of `default_forgetting_half_life_days`
(content forgetting / recency).

`mind.language` (default: resolved from the system locale, `"en"` unless the
primary language code is `ja`) is the app-wide language for cognitive prompts
and deterministic patterns: the affect classifier, cognitive output contract,
compression summarizer, recall-intent keyword lists, and memory-extraction
patterns all follow it unless their per-task override is set.
Existing installs that never set `mind.language` may see prompt and classifier
language change after upgrading: the default was previously hardcoded English
and is now derived from the system locale (Japanese systems get `ja`).
`mind.emotion.classifier_language` and `mind.context.compression_language`
override it per task; empty (the default) means inherit `mind.language`.
Memory extraction follows `mind.language` directly and no longer reads the
classifier setting. The user-facing LLM instruction
strings are loaded at runtime from `assets/lang/{lang}/prompts.json` and the
deterministic patterns from `assets/lang/{lang}/patterns.json`; when a
pack is absent the build falls back to a compile-time embedded pack for the
languages in `ene_config::SUPPORTED_LANGUAGES` (`en`, `ja`), and otherwise to
English. See
[Turns & Sessions](concepts/turn-and-session.md) §3 for details.

The commitment ledger matches incoming commitments against active ones by title
embedding similarity: `commitment_title_similarity_threshold` (default `0.82`)
is the cosine-similarity cutoff above which a rephrased promise supersedes the
existing commitment instead of being registered as a duplicate. With no
embedding provider configured, the ledger falls back to exact normalized-title
matching and this threshold is unused. Matching still loads active ledger rows
into memory once per apply batch; `mind.memory_limits.commitment_active_match_limit`
(default `4096`) caps that list — far above any plausible concurrent
active-commitment count, bounding memory and embedding work if the ledger grows
large. When a list returns exactly the limit the ledger warns that results may
be truncated; raise `mind.memory_limits.commitment_active_match_limit` (or
`ENE_MIND__MEMORY_LIMITS__COMMITMENT_ACTIVE_MATCH_LIMIT`) if matching misses
active commitments. This is one of two operator-configurable memory settings;
everything else under `mind.memory.*` stays at its code default
(`MindMemoryConfig`). The other is the approval-workflow switch below.

### `mind.memory_approval.*` — pre-save candidate approval

```json
{
  "mind": {
    "memory_approval": {
      "require_approval": false
    }
  }
}
```

`require_approval` (default `false`, env:
`ENE_MIND__MEMORY_APPROVAL__REQUIRE_APPROVAL`) switches typed-memory writes
from auto-save to a review-before-save workflow. When `true`, every extracted
candidate that would otherwise be persisted (or would supersede an existing
memory) is parked in the `pending_candidates` queue instead, carrying its
source turn, source quote, extraction reason, confidence, and supersede
target. The queue is surfaced in the CLI (`/memory approval`) and the desktop
Memory Journal, where each candidate can be inspected, edited,
edit-and-approved, approved, or rejected. Approved candidates are persisted
as typed memories with the original conflict target propagated as
`supersedes_id` and the old memory deactivated (`Superseded`), mirroring the
auto-save supersede semantics; rejected ones are discarded. Edits are
validated before any write and resolution is conflict-safe, so a bad edit or
a raced decision never loses the original candidate. Approval and edit
operations carry the active turn id and are emitted as `CandidateChanged`
audit events on the runtime lifecycle bus.

In approval mode, unapproved candidates are excluded from normal recall: they
surface only in the review queue, never in the prompt. Approval-parked
candidates stay excluded even if the mode is later turned off — only
weak-contradiction deferrals ever compete in recall under
`recall_pending_candidate_limit` as described below. The default auto-save
mode is unchanged (`false`). Commitment candidates (the dedicated ledger
path) and explicit user forget / dispute decisions are applied immediately in
both modes.

The memory arbiter decides whether an incoming candidate contradicts an existing
memory of the same kind by comparing the *similarity of their title embeddings*
(#351): `contradiction_title_similarity_threshold` (default `0.82`) is the
cosine-similarity cutoff above which synonymous titles ("職業" vs "仕事",
"住んでいる場所" vs "居住地") are treated as the same subject and checked for
contradiction, instead of being persisted as unrelated duplicates. With no
embedding provider configured, the arbiter falls back to exact normalized-title
matching and this threshold is unused.

The memory arbiter's four decision thresholds are code-defaulted in
`MindMemoryConfig` and are not operator-configurable (#352). Together they
decide when an incoming candidate is persisted, when it *supersedes* (replaces)
an existing contradictory memory, when the existing memory is flagged
*disputed*, and when the decision is deferred to user confirmation:

| Setting | Default | Meaning |
|---|---|---|
| `min_confidence_to_persist` | `0.65` | Minimum candidate confidence to persist at all. |
| `supersede_confidence_delta` | `0.05` | Margin by which a candidate must exceed the existing memory's confidence to supersede it. |
| `semantic_similarity_threshold` | `0.85` | Cosine similarity at/above which two memories are treated as semantic duplicates. |
| `dispute_confidence_gap` | `0.15` | Confidence gap below which a contradictory candidate marks the existing memory disputed instead of superseding or escalating. |

All four are probabilities/ratios and are clamped into `0.0..=1.0` on load.
`semantic_similarity_threshold` in particular depends strongly on the embedding
model's similarity distribution, so re-tune it when the embedding provider is
swapped.

Candidates deferred to user confirmation (`AskConfirmationLater`) sit in
the `pending_candidates` queue. Besides the desktop settings-screen review list,
they also compete in hybrid recall so the character can ask about them when the
topic comes up — surfacing candidates are marked `[unconfirmed]` in the prompt.
`recall_pending_candidate_limit` (default `3`) caps how many compete per turn;
`0` disables the recall path without affecting the settings-screen review list.
The cap is code-tunable in `MindMemoryConfig` and not yet exposed via settings.
Resolved candidates (approved / rejected) stay in the queue as history until
the retention sweep removes them (`mind.memory.pending_candidate_retention`,
code-defaulted to 14 days / 200 rows), and the CLI `/memory approval history`
and the desktop journal's history view list them with their resolution time.

### `plugins.*` — IPC Plugins & MCP Server Connections

Manages out-of-process tool plugins and Model Context Protocol (MCP) servers:

```json
{
  "plugins": {
    "enabled": true,
    "list": {
      "app": { "enable": true },
      "browser": { "enable": true },
      "fs": { "enable": true, "db_quota_mb": 256 },
      "utility": { "enable": true },
      "web": { "enable": true }
    },
    "max_concurrent": 8,
    "parallel_tool_calls_max": 4,
    "mcp_servers": [
      {
        "name": "filesystem",
        "enabled": true,
        "transport": {
          "type": "stdio",
          "command": "npx",
          "args": ["-y", "@modelcontextprotocol/server-filesystem", "/home/user/allowed"]
        }
      }
    ]
  }
}
```

#### `plugins.mcp_servers` — MCP server entries

Each entry declares one MCP server and takes the fields `name` (used verbatim
for routing and tool namespacing), `enabled`, `transport`, and the optional
`env_passthrough` list:

```jsonc
"mcp_servers": [
  {
    "name": "github",
    "enabled": true,
    "transport": {
      "type": "stdio",
      "command": "npx",
      "args": ["-y", "@modelcontextprotocol/server-github"]
    },
    "env_passthrough": ["GITHUB_PERSONAL_ACCESS_TOKEN"]
  },
  {
    "name": "local-dev",
    "enabled": true,
    "transport": {
      "type": "http",
      "url": "https://example.com/mcp",
      "auth_header": "Bearer <token>"
    }
  }
]
```

- `enabled` and `transport` are required; `enabled: false` skips the server.
- `transport.type` is `"stdio"` (spawn `command` with `args` as a child
  process) or `"http"` (connect to `url`, sending `auth_header` as the
  `Authorization` header; a malformed header refuses the connection rather
  than downgrading to unauthenticated).
- Stdio children run with a **cleared environment**: only `PATH`, `HOME`,
  `TMPDIR`, `LANG`, `TZ`, `LD_LIBRARY_PATH` and the Windows essentials are
  forwarded. Everything else — API keys in particular — must be exported in
  the host environment and whitelisted in `env_passthrough`; there is no
  per-server inline `env` map.
- `mcp_servers` is an array, so entries are declared in `settings.json`:
  `ENE_` env vars can override scalar options (e.g.
  `ENE_PLUGINS__MCP_ALLOW_INSECURE_URLS`) but cannot add array elements.

HTTP MCP endpoints validate their URL before connecting (HTTPS-only by default;
loopback and cloud-metadata/link-local addresses refused). Set
`"mcp_allow_insecure_urls": true` inside `plugins` to permit plain-`http://` and
loopback URLs for local development; link-local addresses stay refused. See
[Plugins & MCP](concepts/plugins-and-mcp.md) and the
[MCP Server Setup Guide](guide/tools/mcp-servers.md) for per-service examples
(Calendar, Mail/Chat, Notes, Map, RSS).

`max_concurrent` bounds the number of concurrent in-flight IPC requests **per
plugin connection**, across *all* request types (tool calls, pings,
`list_tools`, `chat_completion`, …) — not just tool calls. Requests beyond the
bound queue (bounded by their own timeout) rather than fanning out to the
plugin. Chat *streams* (`CreateChatStream`) are the exception: they bypass this
bound and are not counted against it.

`parallel_tool_calls_max` bounds how many **side-effect-free** tool calls from a
single LLM response run concurrently. When the model emits several tool calls in
one turn, those whose `ToolSpec` declares `side_effects: ReadOnly` (and which are
not background-capable) are dispatched in parallel up to this bound; everything
else — side-effectful tools, tools that do not declare side effects, and
`system.search_tools` — runs sequentially as before. Parallelism applies only
when **every** call in the response is side-effect-free: in a mixed round a
read-only call must never overtake an earlier write from the same response
(read-after-write), so mixed rounds run strictly sequentially in original
order. Results are re-ordered back
to the original `tool_calls` order, so permission/user-input prompts, the undo
stack, `ToolCallStart`/`ToolCallResult` events, and `ToolResultSummary` ordering
are all preserved. Set it to `0` to disable parallelism entirely and force the
previous fully-sequential behavior. The classification is fail-closed: a tool
that does not declare `ReadOnly` side effects is never parallelized.

`plugins.list.<name>.db_quota_mb` caps how much of the **shared `memory.db`** a
 plugin's tables may occupy, in mebibytes (#424). Stateful plugins write into
 one shared database, so without a cap a single runaway or malicious plugin
 could exhaust the disk or bloat `memory.db` enough to degrade the memory
 system's queries, backups, and integrity checks. The host measures each
 plugin's footprint (the summed byte length of every cell across its declared
 tables) and rejects any storage-growing write — `Insert`/`Upsert`, including
 those inside a `Batch` — that would push it to or past the cap, returning a
 `QUOTA_EXCEEDED` error. Reads and deletes are never gated, so a plugin over
 quota can always free space. The default is `256` — generous enough that no
 built-in plugin comes close, while still bounding a runaway plugin before it
 does real damage. Set the field to `null` to disable enforcement for a plugin
 that legitimately needs unbounded storage.

#### `plugins.list.<name>.config` — plugin-owned settings (#313)

Every plugin entry can carry a host-**opaque** configuration blob:

```json
{
  "plugins": {
    "list": {
      "anthropic": {
        "enable": true,
        "config": {
          "api_key": { "source": "env", "env": "ANTHROPIC_API_KEY" }
        }
      },
      "llama-cpp": {
        "enable": true,
        "config": {
          "mmproj_url": "https://example.com/mmproj.gguf",
          "acceleration": "vulkan"
        }
      }
    }
  }
}
```

The host stores and delivers this blob **verbatim** — it never interprets,
rewrites, or drops keys inside it (unknown keys survive load → save
round-trips). It is sent to the plugin once at handshake time
(`ConfigurablePlugin::set_config`); plugins that also implement provider
traits (LLM/embed/TTS/STT) receive it the same way as tool plugins. The
environment override path for a single key is
`ENE_PLUGINS__LIST__<NAME>__CONFIG__<KEY>`
(e.g. `ENE_PLUGINS__LIST__ANTHROPIC__CONFIG__API_KEY`). Provider-specific
settings that previously lived in `ai.*` moved here — for example
`plugins.list.llama-cpp.config.{mmproj_url,mmproj_path,acceleration}`
(was `ai.local_models.<name>.{mmproj_url,mmproj_path,acceleration}`),
`plugins.list.onnx.config.ort_dylib_path`
(was `ai.ort_dylib_path`), and
`plugins.list.kokoro.profiles.kokoro.voices_path`
(was `ai.tts.voices_path`).

The llama-cpp plugin's `acceleration` value must match the binary's build:
the released Linux packages compile the `vulkan` backend in, while a
CPU-default build (`cargo build -p ene-plugin-llama-cpp` without
`--features vulkan`) rejects `"vulkan"` / `"cuda"` at load with a typed
error. `"auto"` (the default) always works: it selects the compiled GPU
backend when present and falls back to CPU otherwise.

Version-1 `settings.json` files are migrated automatically on load: the
relocated keys above are moved into their `plugins.list.*` destinations (and
removed from their old `ai.*` locations) before the file is read, then the
migrated document is persisted. Files without those keys are left logically
unchanged. Legacy flat entry-level keys (`plugins.list.<name>.<key>`, from
before the nested `config`/`profiles` hierarchy) are also folded into the
delivered config blob at startup, with explicit `config` keys taking
precedence — the file on disk is not rewritten for this, so the fold is
stable across reloads.

Version-2 files are migrated to version 3 on load: every
`ai.local_models.<name>` entry is mirrored into
`plugins.list.llama-cpp.profiles.<name>` (non-empty `url` / `quantization` /
`model_path` / `gpu_layers` / `context_size` / `dimensions` only; a non-empty
existing profile value is never overwritten, and an existing empty value
counts as absent). `ai.local_models` itself is left intact — `ene-ai` still
routes local tasks and budgets context windows from it, and the host reads
the declared embedding dimensions from it. Version-3 files are migrated to
version 4 on load: the same mirror runs again so installs that reached v3
before `context_size` / `dimensions` joined the key set receive them without
a hand edit (existing non-empty profile values are still never overwritten).

#### `plugins.list.<name>.profiles.<profile>` — per-profile settings (#313)

A single plugin can need different settings per model/voice/profile. The
`profiles` map holds host-opaque per-profile blobs, delivered to the plugin at
handshake time (`ConfigurablePlugin::set_profiles`); profile *selection* is
plugin-owned:

```json
{
  "plugins": {
    "list": {
      "kokoro": {
        "enable": true,
        "profiles": {
          "kokoro": { "voices_path": "/data/voices.bin" }
        }
      },
      "llama-cpp": {
        "enable": true,
        "profiles": {
          "gemma-4-e4b": {
            "url": "https://example.com/gemma-4-e4b.gguf",
            "quantization": "Q4_0",
            "model_path": "",
            "gpu_layers": "33",
            "context_size": 16384
          }
        }
      }
    }
  }
}
```

The local GGUF provider plugin (`ene-plugin-llama-cpp`) consumes one profile
per model: `url` (GGUF download URL), `quantization` (label, e.g. `"F16"` /
`"Q4_0"`), `model_path` (local path override that skips download when
non-empty), `gpu_layers` (`"auto"` or an integer string), and the optional
`context_size` (chat context window in tokens; defaults to `16384` when
omitted) plus the optional `dimensions` (declared embedding dimensionality;
see below). The plugin downloads `url` weights into the model cache on first
use (GGUF magic validated) and keeps one loaded model per profile for its
process lifetime. The first download runs inside the proactive decision
warm-up's 5-minute budget; a model that takes longer to fetch fails closed
(proactive stays `Disabled` until the host restarts), so point
`model_path` at a pre-fetched cache for very large models. `context_size`
and `gpu_layers` size chat loads only — the embedding model sizes its own
context and offload plan internally, and the
host's routing window stays in `ai.local_models.<name>.context_size`. The
v2→v3 migration mirrors `context_size`, so a profile that omits it loads the
host-side value (16,384 when the host value is also the default); only
manually written profiles that predate the mirror can drift — set their
`context_size` to at least the host value to avoid a context overflow at
generation time. Profile *selection* is plugin-owned; the values are
delivered via `ConfigurablePlugin::set_profiles`.

`dimensions` is required on `ai.local_models.<name>` when the model backs
`tasks.embedding` with `provider: "local"`: the host opens the memory-store
vector schema with this value before the plugin host starts, and the plugin
rejects a declared value that differs from the model's real dimensionality
on the first `embed_batch` request, when the model is loaded (e.g. `1024`
for the bundled `jina-v5-small` entry). The embedding task's own
`dimensions` field is a cloud-only knob and is ignored for local providers.

#### VOICEVOX / Aivis Speech TTS provider (`plugins.list.voicevox.config`)

The `voicevox` provider plugin (`plugins/provider/voicevox`) speaks the
VOICEVOX HTTP API, so it works with VOICEVOX Engine, Aivis Speech, and other
compatible engines (COEIROINK, …). No API key is needed — the engine is a
local HTTP server. Select it with `ai.tts.provider = "voicevox"`; the generic
`ai.tts.voice` field can optionally hold a speaker/style ID that overrides
the configured default per request.

```json
{
  "ai": {
    "tts": {
      "provider": "voicevox",
      "voice": "14"
    }
  },
  "plugins": {
    "list": {
      "voicevox": {
        "enable": true,
        "config": {
          "server_url": "http://127.0.0.1:50021",
          "speaker_id": 3,
          "speed_scale": 1.0,
          "pitch_scale": 0.0,
          "intonation_scale": 1.0,
          "volume_scale": 1.0,
          "tempo_dynamics_scale": 1.0,
          "output_sampling_rate": 24000,
          "auto_start": false,
          "engine_path": "/opt/voicevox/run.exe",
          "engine_args": ["--port", "50021"],
          "startup_timeout_secs": 10
        }
      }
    }
  }
}
```

Settings:

| Key | Default | Description |
|---|---|---|
| `server_url` | `http://127.0.0.1:50021` | Engine HTTP base URL. VOICEVOX defaults to port 50021; Aivis Speech to 10101. |
| `speaker_id` | `0` | Default speaker / style ID (64-bit integer; Aivis style IDs exceed 32 bits). |
| `speed_scale` | `1.0` | Speech speed multiplier (engine-validated, e.g. 0.5–2.0). |
| `pitch_scale` | `0.0` | Pitch shift (engine-validated, e.g. −0.15–0.15 for VOICEVOX). |
| `intonation_scale` | `1.0` | Intonation strength (engine-validated, e.g. 0–2). |
| `volume_scale` | `1.0` | Output volume (engine-validated, e.g. 0–2). |
| `tempo_dynamics_scale` | `1.0` | Aivis Speech extension: tempo dynamics strength (0–2). Only sent when non-default, since VOICEVOX rejects unknown fields. |
| `output_sampling_rate` | unset | Output sample rate (e.g. 24000/48000). Only sent when set; the engine default applies otherwise. |
| `auto_start` | `false` | Managed mode: spawn the engine binary when the server is not already running. |
| `engine_path` | unset | Engine executable path used by managed mode. |
| `engine_args` | `[]` | Extra command-line arguments passed to the engine binary. |
| `startup_timeout_secs` | `10` | How long managed mode waits for `GET /version` after spawning. |

Every key can be overridden per environment variable as
`ENE_PLUGINS__LIST__VOICEVOX__CONFIG__<KEY>`
(e.g. `ENE_PLUGINS__LIST__VOICEVOX__CONFIG__SPEAKER_ID`).

**External mode (default).** Start the engine yourself — launch the VOICEVOX
app or `run.exe`, or an Aivis Speech / COEIROINK server — and point
`server_url` at it. The plugin calls `POST /audio_query` then
`POST /synthesis` (the standard 2-step flow) and returns WAV audio.

**Managed mode (`auto_start: true`).** On first use the plugin probes
`GET /version`; if no engine answers, it spawns `engine_path` with
`engine_args` and polls `/version` until `startup_timeout_secs` elapses. The
spawned engine is terminated when the plugin process shuts down. The engine
binary must be pre-installed; the plugin never downloads it.

Changing `ai.tts.provider` itself (e.g. switching from `kokoro` to
`voicevox`) takes effect at the next startup: the active provider is built
once at bootstrap, while edits to `plugins.list.voicevox.config` and
`ai.tts.voice` are picked up by running sessions.

#### OpenAI Speech API TTS provider (`plugins.list.openai-tts.config`)

The `openai-tts` provider plugin (`plugins/provider/openai-tts`) synthesizes
speech through the OpenAI Speech API (`tts-1` / `tts-1-hd`) and returns
streaming raw 24 kHz 16-bit mono PCM (`response_format=pcm`). It uses the
same `OPENAI_API_KEY` credential family as the `openai` plugin. Select it
with `ai.tts.provider = "openai_tts"`; the generic `ai.tts.voice` field can
optionally hold a voice name that overrides the configured default per
request.

```json
{
  "ai": {
    "tts": {
      "provider": "openai_tts",
      "voice": "nova"
    }
  },
  "plugins": {
    "list": {
      "openai-tts": {
        "enable": true,
        "config": {
          "api_key": "sk-...",
          "model": "tts-1",
          "voice": "alloy",
          "speed": 1.0,
          "sample_rate": 24000
        }
      }
    }
  }
}
```

Settings:

| Key | Default | Description |
|---|---|---|
| `api_key` | unset | OpenAI API key, or a `{source: inline\|env\|auto}` descriptor. Falls back to the `OPENAI_API_KEY` environment variable. Marked `x-ene-secret`, so the host masks and redacts it. |
| `model` | `tts-1` | Speech synthesis model (`tts-1` for low latency, `tts-1-hd` for higher quality). |
| `voice` | `alloy` | Default voice (`alloy`, `echo`, `fable`, `onyx`, `nova`, `shimmer`); a per-request voice overrides it. |
| `speed` | `1.0` | Speech speed multiplier (0.25–4.0). |
| `sample_rate` | `24000` | Sample rate written into the WAV header. The Speech API's `pcm` format is fixed at 24 kHz, so only set this when `base_url` points at a compatible endpoint with a different output rate. |
| `base_url` | `https://api.openai.com/v1` | API base URL override (for OpenAI-compatible endpoints). Falls back to the `OPENAI_BASE_URL` environment variable. |

The plugin requests `response_format=pcm` from the Speech API and returns
the audio as WAV (16-bit mono PCM at `sample_rate`), which the host-side
audio pipeline decodes into float samples for playback
(`formats = ["wav"]`). Other formats accepted by the Speech API (`mp3`,
`opus`, `flac`, `aac`) are not exposed.

#### Microsoft Edge Neural Voice TTS provider (`plugins.list.edge-tts.config`)

The `edge-tts` provider plugin (`plugins/provider/edge-tts`) talks to
Microsoft's Edge Read Aloud WebSocket endpoint — the same free, keyless
neural voices the browser's read-aloud feature uses. No API key and no local
server are needed. Select it with `ai.tts.provider = "edge-tts"`; the generic
`ai.tts.voice` field can hold an Edge voice name (short form, e.g.
`ja-JP-NanamiNeural`) that overrides the configured default per request.

```json
{
  "ai": {
    "tts": {
      "provider": "edge-tts",
      "voice": "ja-JP-NanamiNeural"
    }
  },
  "plugins": {
    "list": {
      "edge-tts": {
        "enable": true,
        "config": {
          "voice": "ja-JP-NanamiNeural",
          "locale": "ja-JP",
          "rate": "+0%",
          "pitch": "+0Hz",
          "volume": "+0%",
          "max_retries": 3
        }
      }
    }
  }
}
```

Settings:

| Key | Default | Description |
|---|---|---|
| `voice` | `ja-JP-NanamiNeural` | Edge voice name, short (`ja-JP-NanamiNeural`) or long form. |
| `locale` | `ja-JP` | SSML `xml:lang` value on the `<speak>` element. |
| `rate` | `+0%` | Prosody rate adjustment (e.g. `+10%`, `-10%`). |
| `pitch` | `+0Hz` | Prosody pitch adjustment (e.g. `+5Hz`, `-5Hz`). |
| `volume` | `+0%` | Prosody volume adjustment (e.g. `+10%`, `-10%`). |
| `max_retries` | `3` | Reconnect attempts for the whole synthesize request (shared across text chunks), with exponential backoff (0–10). |
| `endpoint_url` | `wss://speech.platform.bing.com/consumer/speech/synthesize/readaloud/edge/v1` | WebSocket endpoint; must not carry a query string. |

The connection mimics the Edge Read Aloud extension (Chrome/Edge user agent,
extension `Origin`, `Sec-MS-GEC` token) and requests
`audio-24khz-48kbitrate-mono-mp3`. Text longer than 4096 bytes is split at
whitespace/UTF-8/XML-entity-safe boundaries and synthesized chunk by chunk
over the same connection; the plugin decodes the MP3 stream and returns WAV
audio (24 kHz mono). If the connection drops, the current chunk is retried
with exponential backoff, up to `max_retries` times in total per request
(the budget is shared across chunks).

Changing `ai.tts.provider` itself (e.g. switching from `voicevox` to
`edge-tts`) takes effect at the next startup: the active provider is built
once at bootstrap, while edits to `plugins.list.edge-tts.config` and
`ai.tts.voice` are picked up by running sessions.

#### Secret marking

A plugin's `config_schema()` may mark a field with `x-ene-secret: true`. The
host uses this (plus a well-known-name fallback: `api_key`, `token`,
`password`, `authorization`, …) to mask the field in the settings UI (planned)
and to redact it from host log output — an inline API key can never appear in
the log stream. Storing secrets outside `settings.json` (a keyring/secret
service) is tracked separately; until then plugin secrets stay in
`plugins.list.<name>.config`, marked by the schema and redacted at the host
boundary.

### `tools.*` — Tool-Execution Runtime Behavior

Distinct from `plugins.*` (which manages the plugin *process*/IPC layer):
`tools.*` covers tool-invocation runtime knobs owned by `ene-runtime` and
`ene-rag`. `tools.rag` configures the Tool RAG selection pipeline
(`ene_rag::ToolRagConfig`); the fields shown below
(`ene_runtime::ToolRuntimeConfig`) cap how many background tasks the turn
actor keeps in flight at once and bound the deferred-tool poll budget. Once
a cap is reached, admission is rejected (fails fast) rather than queued
without bound — `CallTool`/`CancelDeferredTool` and `SearchTools` calls get
back an actionable "busy" error; the post-turn classifier, memory-writer,
and deferred-tool-poller admission points have no reply channel of their
own, so a rejection there only shows up as a `TaskRejected` diagnostic
event:

```json
{
  "tools": {
    "call_tool_cap": 64,
    "deferred_tool_cap": 32,
    "classifier_cap": 16,
    "memory_writer_cap": 16,
    "search_cap": 16,
    "deferred_max_polls": 600,
    "rag": {
      "enabled": true,
      "top_k": 12,
      "min_similarity": 0.20,
      "use_failure_feedback": true,
      "failure_penalty": 0.5,
      "weights": {
        "summary": 1.0,
        "description": 0.6,
        "capability": 0.8,
        "example": 0.4,
        "negative": -0.5,
        "negative_threshold": 0.70
      }
    }
  }
}
```

Tool RAG scores each tool as a weighted average of its per-field embedding
similarities (`[-1, 1]`). `min_similarity` (default `0.20`) is the inclusion
floor for that average; `weights.negative_threshold` (default `0.70`) is the
gate at which a tool's negative-example embedding excludes it outright — a
tool that matches its own negative example this strongly is filtered, not
penalized.

When `use_failure_feedback` (default `true`) is enabled, tools that recently
failed for the active character are down-weighted: their score is multiplied by
`failure_penalty` (default `0.5`, so a failed tool halves its score) before
ranking, and a tool pushed below
`min_similarity` by the penalty is dropped. Recent failures are read through
`ene_core::ToolFailureSignalPort` (implemented by `ene-store`), so the pipeline
stays free of a persistence dependency — see
[Memory System §5](concepts/memory-system.md#5-tool-derived-memory-guardrails).

### `scheduler.*` — Persistent Scheduler Policy

Controls the persistent scheduler (`ene_runtime::scheduler::SchedulerConfig`),
which fires one-shot, interval, cron, and startup schedules. The scheduler
requires the memory store (`store.enabled`); without it no schedule fires.
Schedules and run history live in the store's database and survive restarts.
See the [Schedules guide](guide/schedules.md) for the CLI surface.

```json
{
  "scheduler": {
    "enabled": true,
    "late_grace_secs": 60,
    "confirmation_timeout_secs": 300
  }
}
```

- `enabled` (default `true`) — master switch; when `false` no schedule fires.
  `ENE_SCHEDULER__ENABLED`.
- `late_grace_secs` (default `60`) — a fire processed more than this many
  seconds after its scheduled time (system suspend, clock jump, or the app
  being closed) is recorded `skipped_late` and is **not** executed; the next
  occurrence is computed from the current time. `ENE_SCHEDULER__LATE_GRACE_SECS`.
- `confirmation_timeout_secs` (default `300`) — how long a scheduled run
  awaiting user confirmation may wait before it is recorded `timed_out`.
  `ENE_SCHEDULER__CONFIRMATION_TIMEOUT_SECS`.

### `rag.workspace` — Document/Workspace RAG Settings

Indexes local documents and project folders for citation-bearing retrieval
into conversation context. **Privacy-first defaults: the feature is disabled
and no folder is scanned until the operator explicitly opts in.** Only the
folders listed in `folders` are ever read, and results are restricted to those
same folders at search time. See the
[Workspace RAG guide](guide/workspace-rag.md) for the full privacy model.

```json
{
  "rag": {
    "workspace": {
      "enabled": false,
      "folders": [],
      "include_extensions": [
        "md", "markdown", "txt", "rs", "toml", "json", "yaml", "yml",
        "py", "ts", "js", "tsx", "jsx", "html", "css", "sh", "xml", "ini",
        "cfg", "csv"
      ],
      "ignore_globs": [
        ".git/**", "node_modules/**", "target/**", "dist/**", ".venv/**",
        "**/.env", "**/.env.*", "*.gguf", "*.safetensors", "*.ckpt",
        "*.pth", "*.onnx", "*.bin", "*.db", "*.db-wal", "*.db-shm",
        "assets/models/**"
      ],
      "max_file_bytes": 1048576,
      "chunk_chars": 1200,
      "chunk_overlap_chars": 200,
      "max_chunks_per_file": 256,
      "top_k": 8,
      "final_n": 4,
      "min_similarity": 0.20,
      "sync_on_startup": false
    }
  }
}
```

- `enabled` — master switch. When `false` (default), nothing is scanned,
  searched, or injected into prompts.
- `folders` — the **only** folders that may be scanned and searched. Paths are
  canonicalized before scanning; directory symlinks are never followed and
  file symlinks are indexed only when their canonical target stays inside the
  configured folder; files whose canonical path escapes it are skipped.
- `include_extensions` — file extensions (case-insensitive, no dot) eligible
  for indexing.
- `ignore_globs` — glob patterns (relative to each folder) excluded from
  scanning. Defaults cover version-control metadata, dependency/build
  directories, `.env`-style secrets, model weights (`.gguf`, `.safetensors`,
  ...), and database files. `*` matches within a segment, `**` across
  segments, `?` a single character; patterns without `/` match basenames.
- `max_file_bytes` — files larger than this are skipped entirely (default
  1 MiB).
- `chunk_chars` / `chunk_overlap_chars` — target chunk size and the overlap
  repeated at the next chunk's start (line-granular).
- `max_chunks_per_file` — hard cap; files exceeding it are skipped rather than
  silently truncated.
- `top_k` / `final_n` — vector over-fetch before scoring, and the maximum
  chunks injected into a prompt (or returned by `/workspace search`).
- `min_similarity` — minimum hybrid score for a hit to be returned.
- `sync_on_startup` — run a background sync when the runtime opens (still
  requires `enabled`).

All keys follow the standard `ENE_` override scheme, e.g.
`ENE_RAG__WORKSPACE__ENABLED=true`, `ENE_RAG__WORKSPACE__FOLDERS=/path/a,/path/b`
(JSON-encoded arrays work too).

### `desktop.*` — Desktop GUI & Graphics Parameters

Controls display language, graphics render parameters, microphone input
device, and Beat Sync (system-audio rhythm avatar motion):

```json
{
  "desktop": {
    "language": "en",
    "mic_device": null,
    "beat_sync": {
      "enabled": false,
      "device": null
    },
    "graphics": {
      "vsync": true
    }
  }
}
```

- `desktop.beat_sync.enabled` — capture the system audio loopback and sway
  the avatar on the detected beat. Defaults to `false` (capturing system
  audio by default is a privacy surprise). Requires a PulseAudio / PipeWire
  monitor device exposed as a capture device; without one the feature logs
  and stays disabled.
- `desktop.beat_sync.device` — optional explicit loopback device name.
  When unset, the monitor of the default output device is auto-selected
  (falling back to any input device named "monitor").

See [Voice & Avatar](concepts/voice-and-avatar.md) for the detection
algorithm and platform support details.

---

## 3. Character Card Format (`character.json`)

Ene loads character personalities and prompt templates via JSON character cards:

```json
{
  "name": "Alicia",
  "identity": "Cybernetic artificial intelligence companion living inside the computer.",
  "system_prompt": "You are Alicia, an energetic AI assistant. Answer directly and concisely.",
  "greeting": "Systems operational. What are we working on today?",
  "initial_affect": {
    "pleasure": 0.6,
    "arousal": 0.7,
    "dominance": 0.5
  }
}
```

### Character resolution and discovery

A character lives in a folder under `assets/characters/` and its card file is
`character.json`:

```
assets/characters/<name>/
  character.json
  character_settings.json
  model.vrm
  motions/VRMA_01.vrma
```

- The `character` setting is a **bare name** (`"Alicia"`), resolved to
  `assets/characters/Alicia/character.json`, or a **card path** (relative or
  absolute, e.g. `assets/cards/ene.json`).
- Discovery (`ene characters list`, the desktop character picker) uses the
  same rule: a folder counts as a character only when it contains
  `character.json`. The legacy misspelled `charactor.json` filename is no
  longer accepted.
- **Unset vs. missing.** An empty `character` value is a distinct error
  ("no character selected"); it no longer silently falls back to a hardcoded
  default. A non-empty name whose card file is absent reports the missing
  file instead.
- **Path validation.** `..` traversal components are rejected, since
  character names come from third-party card distributions.

---

## 4. Schema Generation

Settings schemas are declared at each owning crate via `define_config!`. Schemas are written once per process at application startup (CLI `init`, desktop first-launch, and the runtime open paths), not on every config load. Each schema file is written atomically (temp file + `fsync` + rename), so a crash can never leave a truncated schema behind.

When `settings.json` is saved, Ene automatically writes a relative `$schema` pointer (`./schema/settings.schema.json`) at the top of the file so editors provide completions and validation without the key being hand-written. An existing `$schema` value is preserved verbatim; the pointer is only filled in when it is absent. The user's hand-arranged top-level section order is likewise preserved across a save, and newly added sections append at the end.

> [!CAUTION]
> Never hand-edit or commit ignored schema files under `assets/schema/*`.
