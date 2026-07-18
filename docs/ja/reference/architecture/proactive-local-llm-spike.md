# スパイク: AMD Vulkan 上の Gemma 4 E2B/E4B + llama-server (#170)

- **Date:** 2026-07-18
- **Parent:** [#165](https://github.com/pexisgle/ene/issues/165)
- **Epic:** [#103](https://github.com/pexisgle/ene/issues/103)

英語版と同期: [proactive-local-llm-spike.md](../../../reference/architecture/proactive-local-llm-spike.md)

## このマシンでの確認結果

| 項目 | 結果 |
|---|---|
| OS | Linux（NixOS / flake `direnv`） |
| Vulkan | あり（RADV）。非適合警告あり |
| `llama-server` | **未インストール** |
| 環境変数 / GGUF | 未設定 / リポジトリに含めない |

実機での GPU offload + Gemma 4 decision JSON は、外部 binary と GGUF が無いため未完了。#165 は env 付き smoke を opt-in とし、欠落時は fail-closed（`fallback=disabled` なら黙ってクラウドへ送らない）。

## #165 向け暫定ピン

| 要件 | 値 |
|---|---|
| Binary | Vulkan 対応かつ Gemma 4 chat template 対応の `llama-server` |
| Bind | `--host 127.0.0.1` のみ |
| AMD Vulkan | `--device Vulkan0`（または同等） |
| Offload | `--n-gpu-layers`；Vulkan 要求なのに CPU-only なら失敗扱い |
| Context | 判定用に小さく（≈2048） |
| Parallel | `1` |
| Structured output | JSON schema 優先、不可なら厳格 parse + fail-closed |

成功スモーク時に revision / GGUF hash / ドライバ情報をこの文書へ追記すること。
