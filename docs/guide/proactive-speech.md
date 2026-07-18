# Proactive companion speech — local decision model

Proactive speech (#103) uses a **lightweight decision model** and the normal chat model for utterances. Decision backends:

| `provider.proactive.decision.backend` | Behaviour |
|---|---|
| `disabled` (default) | No decisions; feature stays silent even if `mind.proactive.enabled` |
| `llama_cpp` | In-process llama-cpp-2 load of a GGUF on `model_path` |
| `cloud` | Uses `provider.base_url` / `api_key` with optional `cloud_model` |

## Enabling

1. Set `mind.proactive.enabled` to `true`.
2. Configure sources under `mind.proactive.sources`.
3. Set `provider.proactive.decision.backend` to `llama_cpp` or `cloud`.
4. For local: set `model_path` to a Gemma 4 E2B/E4B (or other) GGUF. Optional `acceleration` / `gpu_layers` for Vulkan/CUDA.
5. Optional: set `provider.proactive.generation_model` to use a different chat model for proactive utterances.

Desktop settings apply to the running actor immediately (no restart). The desktop observer and runtime scheduler both receive updates via `UpdateProactiveSettings`.

Weights are **not** bundled. Local inference is **in-process llama-cpp-2** (no `llama-server` subprocess). See the [ADR](../reference/architecture/proactive-speech.md).

## Smoke test (optional)

```bash
export ENE_LOCAL_LLM_MODEL=/path/to/model.gguf
export ENE_LOCAL_LLM_BACKEND=vulkan   # or cuda / cpu
direnv exec . rtk cargo test -p ene-ai --lib local_llm::routing::smoke
```

## Privacy

- Desktop never writes raw screenshots to disk, logs, or SQLite.
- Screen summary is optional; when enabled, V1 desktop reports the source as **unavailable** until a summarizer is integrated (no silent empty summaries).
- Activity uses **application name only** (no raw window titles; no keylogging).

## Desktop integration

- Proactive turns emit `EneEvent::TurnStarted` before streaming; desktop sets `active_turn` from this event so `TextDelta` / `Terminal` reach the chat UI.
- Cooldown and session caps apply only after a proactive turn ends with `TerminalReason::Done` (failed or cancelled generations do not consume cooldown).
- Local llama-cpp embedding and decision paths share a process-wide inference lock (serialized, not concurrent).
