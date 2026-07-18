# Spike: Gemma 4 E2B/E4B on AMD Vulkan via llama-cpp-2 (#170 / #171)

- **Date:** 2026-07-18
- **Parent:** [#165](https://github.com/pexisgle/ene/issues/165)
- **Compare:** [#171](https://github.com/pexisgle/ene/issues/171)
- **Epic:** [#103](https://github.com/pexisgle/ene/issues/103)

## Decision (#171)

**Adopt A: embed `llama-cpp-2` in `ene-ai`.** Reject B (`llama-server` child process).

| Criterion | Outcome |
|---|---|
| No external binary / loopback HTTP | A wins |
| Vulkan device + layer offload from Rust | A wins (`list_llama_ggml_backend_devices`, `with_n_gpu_layers`) |
| Structured decision via grammar | A wins (`json_schema_to_grammar`) |
| Candle for chat / AMDGPU | Out of scope; Candle **removed** — local embedding also uses llama-cpp-2 |

Candle is no longer used anywhere in `ene-ai`. Local embedding and proactive decision share one native backend.

## Environment checked (this machine)

| Item | Result |
|---|---|
| OS | Linux (NixOS / flake `direnv`) |
| Vulkan | Present (`vulkaninfo`); RADV selectable |
| Build | `clang` + `cmake` + `LIBCLANG_PATH` in flake; `ene-ai` feature `vulkan` on Linux desktop |
| Gemma 4 GGUF in repo | **Not present** (by design — never commit weights) |

Full GPU offload + Gemma 4 decision JSON still needs an operator-supplied GGUF. Local smoke is **opt-in** via env vars. Fail-closes when the model is missing (no silent cloud upload when `fallback=disabled`).

## Provisional pin for #165 / #171

| Requirement | Value |
|---|---|
| Crate | `llama-cpp-2` (workspace pin; tracks upstream llama.cpp via `llama-cpp-sys-2`) |
| Device (AMD Vulkan) | Prefer Vulkan backend device from `list_llama_ggml_backend_devices` |
| Offload | `n_gpu_layers` large / `auto`; treat as failure if Vulkan requested but no Vulkan device |
| Context | Small (`context_size` ≈ 2048 for decisions) |
| Structured output | Prefer `json_schema_to_grammar` + greedy sample; otherwise strict parse + fail-closed |

**Do not** auto-fallback to cloud when Vulkan offload fails unless `provider.proactive.decision.fallback = cloud`.

## Verification commands (operator)

```bash
export ENE_LOCAL_LLM_MODEL=/path/to/gemma-4-e2b-or-e4b.gguf
export ENE_LOCAL_LLM_BACKEND=vulkan

direnv exec . rtk cargo test -p ene-ai --lib local_llm::routing::smoke
```

Record for a successful smoke:

- `llama-cpp-2` / llama.cpp revision from Cargo.lock
- GGUF distribution URL, hash, quantization, license
- GPU name + RADV / driver version
- Whether model chat template metadata was present
- Whether JSON schema grammar worked
- Log lines proving Vulkan device selection and layer offload

## Fail-closed inputs for Ene

| Condition | Ene behaviour |
|---|---|
| Model path missing / empty | Typed error; no speech |
| Model load / OOM / native error | Typed error; no speech |
| Request timeout | Typed error; no speech |
| Vulkan requested but no Vulkan device | Acceleration failure → configured fallback only |
| Malformed decision JSON | `should_speak=false` |

## Embedding note

Switching local embedding from Candle’s custom graph to llama.cpp **last-token pooling** may change vector spaces. Re-index local sqlite-vec data after upgrading if you use `provider.embedding.backend = local`.

## Follow-up

Re-run this spike when `ENE_LOCAL_LLM_MODEL` fixtures are available and paste results into this document. Until then, CI runs mock provider tests only.
