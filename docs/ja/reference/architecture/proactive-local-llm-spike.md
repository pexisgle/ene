# スパイク: AMD Vulkan 上の Gemma 4 E2B/E4B + llama-cpp-2 (#170 / #171)

- **日付:** 2026-07-18
- **親:** [#165](https://github.com/pexisgle/ene/issues/165)
- **比較:** [#171](https://github.com/pexisgle/ene/issues/171)
- **Epic:** [#103](https://github.com/pexisgle/ene/issues/103)

## 決定 (#171)

**A案を採用: `llama-cpp-2` を `ene-ai` に直接組み込む。** B案（`llama-server` 子プロセス）は却下。

| 基準 | 結果 |
|---|---|
| 外部 binary / loopback HTTP が不要 | A が有利 |
| Rust から Vulkan device + layer offload | A が有利 |
| grammar による structured decision | A が有利 |
| Candle の chat / AMDGPU | 範囲外。Candle は **削除** — ローカル embedding も llama-cpp-2 |

`ene-ai` から Candle 依存はなくなった。ローカル embedding と proactive decision は同一ネイティブ backend を共有する。

## 環境（このマシン）

| 項目 | 結果 |
|---|---|
| OS | Linux (NixOS / flake `direnv`) |
| Vulkan | あり（`vulkaninfo`）；RADV 選択可 |
| Build | flake に `clang` + `cmake` + `LIBCLANG_PATH`；Linux desktop で `ene-ai/vulkan` |
| リポジトリ内 Gemma 4 GGUF | **なし**（重みはコミットしない） |

GPU offload + Gemma 4 decision JSON はオペレータ供給の GGUF が必要。ローカル smoke は env で **opt-in**。モデル欠如時は fail-closed（`fallback=disabled` のとき黙ってクラウドへ送らない）。

## #165 / #171 の暫定ピン

| 要件 | 値 |
|---|---|
| Crate | `llama-cpp-2`（workspace pin） |
| Device (AMD Vulkan) | `list_llama_ggml_backend_devices` から Vulkan を選択 |
| Offload | `n_gpu_layers` 大 / `auto`；Vulkan 要求なのに device 無しなら失敗 |
| Context | 小さめ（判定は `context_size` ≈ 2048） |
| Structured output | `json_schema_to_grammar` + greedy を優先 |

Vulkan offload 失敗時にクラウドへ自動フォールバックしない（`fallback = cloud` のときのみ）。

## 検証コマンド（オペレータ）

```bash
export ENE_LOCAL_LLM_MODEL=/path/to/gemma-4-e2b-or-e4b.gguf
export ENE_LOCAL_LLM_BACKEND=vulkan

direnv exec . rtk cargo test -p ene-ai --lib local_llm::routing::smoke
```

成功時に記録するもの: Cargo.lock の revision、GGUF URL/hash/量子化/ライセンス、GPU + RADV、chat template、grammar、Vulkan offload ログ。

## Fail-closed

| 条件 | 挙動 |
|---|---|
| model_path 欠如 | typed error；発話なし |
| load / OOM / native error | typed error；発話なし |
| timeout | typed error；発話なし |
| Vulkan 要求だが device なし | 設定された fallback のみ |
| 不正な decision JSON | `should_speak=false` |

## Embedding 注意

Candle 自前グラフから llama.cpp の **last-token pooling** へ移行するとベクトル空間が変わり得る。`provider.embedding.backend = local` 利用時はアップグレード後に sqlite-vec を再インデックスすること。

## フォローアップ

`ENE_LOCAL_LLM_MODEL` fixture が用意できたら再実行し、結果をこの文書へ追記する。それまで CI は mock のみ。
