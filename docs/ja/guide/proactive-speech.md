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

重みはアプリに **同梱しない**（外部 `llama-server` も不要）。[スパイク](../reference/architecture/proactive-local-llm-spike.md) と [ADR](../reference/architecture/proactive-speech.md) を参照。

## スモーク（任意）

```bash
export ENE_LOCAL_LLM_MODEL=/path/to/model.gguf
export ENE_LOCAL_LLM_BACKEND=vulkan   # or cuda / cpu
direnv exec . rtk cargo test -p ene-ai --lib local_llm::routing::smoke
```

## プライバシー

- Desktop は生スクリーンショットをディスク・ログ・SQLite に書かない。
- 画面要約は任意。要約器が無い場合はそのソースを利用不可として扱う。
- アクティビティはプライバシー安全なアクティブウィンドウラベルのみ（キーロギングなし）。
