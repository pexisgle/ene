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

Each provider entry may set an optional `context_window` (integer, tokens) to
cap the context window the backend advertises (#364). The effective window is
`min(advertised, context_window)`, so an override can only *shrink* a model's
stated limit, never exceed it; omit it to defer entirely to the provider. A
plugin provider advertises its window through `LlmProviderSpec.context_window`,
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
      "sources": {
        "window_title_level": "app_only"
      }
    },
    "memory": {
      "recall_min_score": 0.10,
      "recall_similarity_threshold": 0.35,
      "commitment_boost": 0.25,
      "commitment_title_similarity_threshold": 0.82,
      "contradiction_title_similarity_threshold": 0.82,
      "min_confidence_to_persist": 0.65,
      "supersede_confidence_delta": 0.05,
      "semantic_similarity_threshold": 0.85,
      "dispute_confidence_gap": 0.15
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

Memory recall uses a hybrid score `(relevance × quality + commitment_boost) × penalty`
(see `crates/ene-rag/src/scoring.rs`). A fresh, strongly relevant memory scores
near `1.0`; recent/lexical-only candidates land around `0.1–0.5`; unrelated
noise scores `0.0`. `recall_min_score` (default `0.10`) filters the final
ranking, `recall_similarity_threshold` (default `0.35`) gates the vector-gather
step, and `commitment_boost` (default `0.25`) lets active promises surface even
with zero query relevance.

`mind.emotion.classifier_language` (default `"en"`) selects the prompt-library
language used for the affect classifier and the cognitive output contract, and
drives the deterministic forget-pattern pack. The user-facing LLM instruction
strings are loaded at runtime from `assets/lang/{lang}/prompts.json` and the
deterministic forget regexes from `assets/lang/{lang}/patterns.json`; when a
pack is absent the build falls back to a compile-time embedded pack for the
languages in `ene_config::SUPPORTED_LANGUAGES` (`en`, `ja`), and otherwise to
English. See
[Turns & Sessions](concepts/turn-and-session.md) §3 for details.

The commitment ledger matches incoming commitments against active ones by title
embedding similarity (#387): `commitment_title_similarity_threshold` (default
`0.82`) is the cosine-similarity cutoff above which a rephrased promise
supersedes the existing commitment instead of being registered as a duplicate.
With no embedding provider configured, the ledger falls back to exact
normalized-title matching and this threshold is unused.

The memory arbiter decides whether an incoming candidate contradicts an existing
memory of the same kind by comparing the *similarity of their title embeddings*
(#351): `contradiction_title_similarity_threshold` (default `0.82`) is the
cosine-similarity cutoff above which synonymous titles ("職業" vs "仕事",
"住んでいる場所" vs "居住地") are treated as the same subject and checked for
contradiction, instead of being persisted as unrelated duplicates. With no
embedding provider configured, the arbiter falls back to exact normalized-title
matching and this threshold is unused.

The memory arbiter's four decision thresholds are all configurable under
`mind.memory.*` (#352). Together they decide when an incoming candidate is
persisted, when it *supersedes* (replaces) an existing contradictory memory,
when the existing memory is flagged *disputed*, and when the decision is
deferred to user confirmation:

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
        "command": "npx",
        "args": ["-y", "@modelcontextprotocol/server-filesystem", "/home/user/allowed"]
      }
    ]
  }
}
```

HTTP MCP endpoints validate their URL before connecting (HTTPS-only by default;
loopback and cloud-metadata/link-local addresses refused). Set
`"mcp_allow_insecure_urls": true` inside `plugins` to permit plain-`http://` and
loopback URLs for local development; link-local addresses stay refused. See
[Plugins & MCP](concepts/plugins-and-mcp.md).

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

### `desktop.*` — Desktop GUI & Graphics Parameters

Controls display language, graphics render parameters, and microphone input device:

```json
{
  "desktop": {
    "language": "en",
    "mic_device": null,
    "graphics": {
      "vsync": true
    }
  }
}
```

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

---

## 4. Schema Generation

Settings schemas are declared at each owning crate via `define_config!`. Schemas are written once per process at application startup (CLI `init`, desktop first-launch, and the runtime open paths), not on every config load. Each schema file is written atomically (temp file + `fsync` + rename), so a crash can never leave a truncated schema behind.

When `settings.json` is saved, Ene automatically writes a relative `$schema` pointer (`./schema/settings.schema.json`) at the top of the file so editors provide completions and validation without the key being hand-written. An existing `$schema` value is preserved verbatim; the pointer is only filled in when it is absent. The user's hand-arranged top-level section order is likewise preserved across a save, and newly added sections append at the end.

> [!CAUTION]
> Never hand-edit or commit ignored schema files under `assets/schema/*`.
