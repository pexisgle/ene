# 能動発話 — ローカル判定モデル

能動発話（#103）は **軽量な判定モデル** と通常のチャットモデルで発話します。

## 有効化

1. `mind.proactive.enabled` を `true` にする。
2. `ai.local_models` に判定モデルエントリを追加する（例: HTTPS `.gguf` URL 付き `gemma-4-e2b`）。
3. `ai.tasks.proactive` を `provider: "local"`、`model` をそのレジストリキーに設定する（`null` のままなら生成のみ `tasks.chat` を再利用）。
4. 任意: 既にローカルにある場合は `local_models` エントリの `model_path` を指定。
5. 任意: Vulkan/CUDA 用に `acceleration` / `gpu_layers` を設定。

例（`assets/settings.json`）:

```json
{
  "ai": {
    "local_models": {
      "gemma-4-e2b": {
        "url": "https://huggingface.co/google/gemma-4-E2B-it-qat-q4_0-gguf/resolve/main/gemma-4-E2B_q4_0-it.gguf",
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

デスクトップ設定は再起動なしで実行中のアクターに反映されます。デスクトップオブザーバーとランタイムスケジューラは `UpdateProactiveSettings` 経由で更新を受け取ります。

重みは **同梱されません**。起動時に並列ダウンロードされ、`[GgufDownload]` で進捗がログ出力されます。ローカル推論は **プロセス内 llama-cpp-2**（`llama-server` サブプロセス不要）。[ADR](../reference/architecture/proactive-speech.md) を参照。

## スモークテスト（任意）

```bash
export ENE_LOCAL_LLM_MODEL=/path/to/model.gguf
export ENE_LOCAL_LLM_BACKEND=vulkan   # or cuda / cpu
direnv exec . rtk cargo test -p ene-ai --lib local_llm::routing::smoke
```

## プライバシー

- デスクトップは生のスクリーンショットをディスク・ログ・SQLite に書き込みません。
- 画面サマリーは任意。有効時、V1 デスクトップはサマライザー統合までソースを **利用不可** と報告します（空サマリーを黙って送りません）。
- アクティビティは **アプリ名のみ**（生のウィンドウタイトルなし、キーロギングなし）。

## デスクトップ統合

- 能動ターンはストリーミング前に `EneEvent::TurnStarted` を発行。デスクトップはこのイベントから `active_turn` を設定し、`TextDelta` / `Terminal` がチャット UI に届きます。
- クールダウンとセッション上限は、能動ターンが `TerminalReason::Done` で終了した後にのみ適用（失敗・キャンセルした生成はクールダウンを消費しません）。
- ローカル llama-cpp の埋め込みと判定パスはプロセス全体の推論ロックを共有（直列、同時実行なし）。
