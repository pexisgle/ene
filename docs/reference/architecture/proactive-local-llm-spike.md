# Spike: Gemma 4 E2B/E4B on AMD Vulkan llama-server (#170)

- **Date:** 2026-07-18
- **Parent:** [#165](https://github.com/pexisgle/ene/issues/165)
- **Epic:** [#103](https://github.com/pexisgle/ene/issues/103)

## Environment checked (this machine)

| Item | Result |
|---|---|
| OS | Linux (NixOS / flake `direnv`) |
| Vulkan | Present (`vulkaninfo` Instance 1.4.350); RADV warned as non-conformant but selectable |
| `llama-server` on PATH | **Not installed** |
| `ENE_LOCAL_LLM_BIN` / `ENE_LOCAL_LLM_MODEL` | Unset |
| Gemma 4 GGUF in repo | **Not present** (by design — never commit weights) |

Full GPU offload + Gemma 4 decision JSON could not be executed in-tree without an external binary and GGUF. The implementation (#165) therefore:

1. Treats local smoke as **opt-in** via env vars.
2. Pins **provisional** CLI assumptions below; operators must re-validate against `llama-server --help` for their build.
3. Fail-closes when the local server is missing (no silent cloud upload when `fallback=disabled`).

## Provisional pin for #165

Until a successful AMD Vulkan smoke is recorded with concrete hashes, Ene treats the following as the **minimum operator checklist** (not a hard git submodule pin):

| Requirement | Value |
|---|---|
| Binary | `llama-server` from llama.cpp with **Vulkan** build and **Gemma 4** chat-template support |
| Bind | `--host 127.0.0.1` only |
| Device (AMD Vulkan) | `--device Vulkan0` (or revision-equivalent) |
| Offload | `--n-gpu-layers` set to a large value / auto-resolved max; treat as failure if logs show CPU-only after requesting Vulkan |
| Context | Small (`--ctx-size` ≈ 2048 for decisions) |
| Parallel | `--parallel 1` |
| UI | `--no-webui` when supported |
| Structured output | Prefer JSON schema / JSON object mode; otherwise strict parse + fail-closed |

**Do not** auto-fallback to cloud when Vulkan offload fails unless `provider.proactive.decision.fallback = cloud`.

## Verification commands (operator)

```bash
# Build or install llama.cpp with Vulkan, then:
export ENE_LOCAL_LLM_BIN=/path/to/llama-server
export ENE_LOCAL_LLM_MODEL=/path/to/gemma-4-e2b-or-e4b.gguf
export ENE_LOCAL_LLM_BACKEND=vulkan

"$ENE_LOCAL_LLM_BIN" --version
"$ENE_LOCAL_LLM_BIN" --help | tee /tmp/llama-server-help.txt

"$ENE_LOCAL_LLM_BIN" \
  --model "$ENE_LOCAL_LLM_MODEL" \
  --host 127.0.0.1 \
  --port 8080 \
  --device Vulkan0 \
  --n-gpu-layers 99 \
  --ctx-size 2048 \
  --parallel 1 \
  --no-webui
```

Health and decision probe:

```bash
curl -s http://127.0.0.1:8080/health
curl -s http://127.0.0.1:8080/v1/chat/completions \
  -H 'Content-Type: application/json' \
  -d '{
    "messages": [
      {"role":"system","content":"Return JSON only with should_speak, confidence, reason, topic_hint, urgency."},
      {"role":"user","content":"seconds_since_user_input: 300\nactivity: idle"}
    ],
    "temperature": 0
  }'
```

Record for a successful smoke:

- llama.cpp git revision / package version
- GGUF distribution URL, hash, quantization, license
- GPU name + RADV / driver version
- Whether `--chat-template gemma` (or metadata auto template) was required
- Whether JSON schema mode worked
- Log lines proving Vulkan device selection and layer offload

## Fail-closed inputs for Ene

| Condition | Ene behaviour |
|---|---|
| Binary missing / not executable | Typed error; no speech |
| Model path missing | Typed error; no speech |
| Health timeout / loading forever | Typed error; no speech |
| Process crash mid-request | Typed error; no speech |
| Vulkan requested but CPU-only in logs | Treat as acceleration failure → configured fallback only |
| Malformed decision JSON | `should_speak=false` |

## Follow-up

Re-run this spike when `ENE_LOCAL_LLM_*` fixtures are available and paste results into this document (revision hash + command that worked). Until then, CI runs mock provider tests only.
