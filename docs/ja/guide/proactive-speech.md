# 能動発話 — ローカル判定モデル

能動発話 (#103) は **軽量な判定モデル** と通常のチャットモデルを使い分けます。

| `provider.proactive.decision.backend` | 挙動 |
|---|---|
| `disabled`（デフォルト） | 判定しない |
| `llama_cpp` | loopback 上で `llama-server` を起動 |
| `cloud` | 既存の `provider.base_url` / `api_key`（任意で `cloud_model`） |

## 有効化

1. `mind.proactive.enabled = true`
2. `mind.proactive.sources` を設定
3. `provider.proactive.decision.backend` を `llama_cpp` または `cloud`
4. ローカル時は GGUF の `model_path` と任意で `executable`

実行ファイルと重みは同梱しません。[スパイク](../reference/architecture/proactive-local-llm-spike.md) と [ADR](../reference/architecture/proactive-speech.md) を参照。

## プライバシー

- raw screenshot はディスク・ログ・SQLite に書きません
- 画面要約が無い環境では source unavailable として扱います
- 活動観測は privacy-safe なウィンドウラベルのみ（キーログなし）
