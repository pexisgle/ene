# Proactive companion speech — local decision model

Proactive speech (#103) uses a **lightweight decision model** and the normal chat model for utterances.

## Enabling

1. Set `mind.proactive.enabled` to `true`.
2. Add a decision model entry under `ai.local_models` (for example `gemma-4-e2b` with an HTTPS `.gguf` URL).
3. Point `ai.tasks.proactive` at `provider: "local"` with `model` set to that registry key (or leave `null` to reuse `tasks.chat` for generation only).
4. Optional: set `model_path` on the local model entry when you already have the weights on disk.
5. Optional: set `acceleration` / `gpu_layers` for Vulkan/CUDA.

Example (`assets/settings.json`):

```json
{
  "ai": {
    "local_models": {
      "gemma-4-e2b": {
        "url": "https://huggingface.co/unsloth/gemma-4-E2B-it-GGUF/resolve/main/gemma-4-E2B-it-Q4_0.gguf",
        "mmproj_url": "https://huggingface.co/unsloth/gemma-4-E2B-it-GGUF/resolve/main/mmproj-F16.gguf",
        "acceleration": "auto",
        "gpu_layers": "auto",
        "context_size": 2048
      }
    },
    "tasks": {
      "proactive": { "provider": "local", "model": "gemma-4-e2b" }
    }
  },
  "mind": {
    "proactive": { "enabled": true }
  }
}
```

Desktop settings apply to the running actor immediately (no restart). The desktop observer and runtime scheduler both receive updates via `UpdateFeatureSettings` (Features tab) / `UpdateProactiveSettings`.

At the default desktop `info` log level you should see:

- `Proactive decision provider ready` — first successful decision-backend init
- `Proactive decision started` — each decision tick that runs gates / LLM
- `Proactive will speak` / `Proactive will not speak` — outcome (`speak`, `detail`, optional `confidence` / `topic_hint`)

Weights are **not** bundled. Downloads run in parallel at startup with `[GgufDownload]` progress logs. Local inference is **in-process llama-cpp-2** (no `llama-server` subprocess). See the [ADR](../reference/architecture/proactive-speech.md).

## Smoke test (optional)

```bash
export ENE_LOCAL_LLM_MODEL=/path/to/model.gguf
export ENE_LOCAL_LLM_BACKEND=vulkan   # or cuda / cpu
direnv exec . rtk cargo test -p ene-ai --lib local_llm::routing::smoke
```

## Privacy

- Desktop never writes raw screenshots to disk, logs, or SQLite (portal temp files are deleted immediately).
- When `sources.screen_summary` is enabled, desktop captures the active window (or primary display), summarizes it with the **local** proactive GGUF + `mmproj` (Gemma 4 multimodal), then discards the image. Only truncated text enters the decision context. If vision fails, the screen source is marked unavailable (no fabricated summary).
- When `tasks.proactive` is `provider: "local"`, a missing or failed GGUF load disables the decision backend — it does **not** fall back to cloud with observation context.
- Activity uses **application name only** (no raw window titles; no keylogging).

## Desktop integration

- Proactive turns emit `EneEvent::TurnStarted` before streaming; desktop sets `active_turn` from this event so `TextDelta` / `Terminal` reach the chat UI.
- Cooldown and session caps apply only after a proactive turn ends with `TerminalReason::Done` (failed or cancelled generations do not consume cooldown).
- Local llama-cpp embedding and decision paths share a process-wide inference lock (serialized, not concurrent).
