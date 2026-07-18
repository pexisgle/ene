# 能動発話 — ローカル判定モデル

能動発話（#103）は **軽量な判定モデル** と通常チャットモデルで発話します。判定バックエンド:

| `provider.proactive.decision.backend` | 挙動 |
|---|---|
| `disabled`（既定） | 判定なし。`mind.proactive.enabled` でも沈黙 |
| `llama_cpp` | プロセス内 llama-cpp-2 で `model_path` の GGUF を読み込む |
| `cloud` | `provider.base_url` / `api_key` と任意の `cloud_model` |

## 有効化

1. `mind.proactive.enabled` を `true` にする。
2. `mind.proactive.sources` を設定する。
3. `provider.proactive.decision.backend` を `llama_cpp` または `cloud` にする。
4. ローカル時は GGUF の `model_path`。任意で `acceleration` / `gpu_layers`（Vulkan/CUDA）。
5. 任意: `provider.proactive.generation_model` で能動発話専用のチャットモデルを指定。

重みは **同梱しない**。ローカル推論は **プロセス内 llama-cpp-2**（`llama-server` 子プロセスなし）。[スパイク](../reference/architecture/proactive-local-llm-spike.md) と [ADR](../reference/architecture/proactive-speech.md) を参照。

## スモーク（任意）

```bash
export ENE_LOCAL_LLM_MODEL=/path/to/model.gguf
export ENE_LOCAL_LLM_BACKEND=vulkan   # or cuda / cpu
direnv exec . rtk cargo test -p ene-ai --lib local_llm::routing::smoke
```

## プライバシー

- Desktop は生スクリーンショットをディスク・ログ・SQLite に書かない。
- 画面要約は任意。有効時も V1 デスクトップは要約器未同梱のため **unavailable** を報告する（空要約を黙って送らない）。
- アクティビティは **アプリ名のみ**（生ウィンドウタイトルなし。キーロギングなし）。
