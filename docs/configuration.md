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
available.

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
active commitments. This is the **only** operator-configurable memory setting —
everything else under `mind.memory.*` stays at its code default
(`MindMemoryConfig`).

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

Version-1 `settings.json` files are migrated automatically on load: the
relocated keys above are moved into their `plugins.list.*` destinations (and
removed from their old `ai.*` locations) before the file is read, then the
migrated document is persisted. Files without those keys are left logically
unchanged. Legacy flat entry-level keys (`plugins.list.<name>.<key>`, from
before the nested `config`/`profiles` hierarchy) are also folded into the
delivered config blob at startup, with explicit `config` keys taking
precedence — the file on disk is not rewritten for this, so the fold is
stable across reloads.

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
      }
    }
  }
}
```

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
