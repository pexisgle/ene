# Proactive companion speech — local decision model

Proactive speech (#103) uses a **lightweight decision model** and the normal chat model for utterances. Decision backends:

| `provider.proactive.decision.backend` | Behaviour |
|---|---|
| `disabled` (default) | No decisions; feature stays silent even if `mind.proactive.enabled` |
| `llama_cpp` | Spawns `llama-server` on `127.0.0.1` only |
| `cloud` | Uses `provider.base_url` / `api_key` with optional `cloud_model` |

## Enabling

1. Set `mind.proactive.enabled` to `true`.
2. Configure sources under `mind.proactive.sources`.
3. Set `provider.proactive.decision.backend` to `llama_cpp` or `cloud`.
4. For local: set `model_path` to a Gemma 4 E2B/E4B (or other) GGUF and optionally `executable` to `llama-server`.

Binary and weights are **not** bundled. See [spike notes](../reference/architecture/proactive-local-llm-spike.md) and the [ADR](../reference/architecture/proactive-speech.md).

## Smoke test (optional)

```bash
export ENE_LOCAL_LLM_BIN=/path/to/llama-server
export ENE_LOCAL_LLM_MODEL=/path/to/model.gguf
export ENE_LOCAL_LLM_BACKEND=vulkan   # or cuda / cpu
direnv exec . rtk cargo test -p ene-ai --lib local_llm::routing::smoke
```

## Privacy

- Desktop never writes raw screenshots to disk, logs, or SQLite.
- Screen summary is optional; when no summarizer is available the source is treated as unavailable.
- Activity uses privacy-safe active-window labels only (no keylogging).
