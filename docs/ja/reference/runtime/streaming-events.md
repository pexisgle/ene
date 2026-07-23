# ストリーミングイベント：Mind ランタイム

`ene-runtime` のアクターは、すべての `EneCommand::Run` を mind ストリーミングパイプラインへディスパッチします（[`ene-runtime` APIリファレンス § ストリーミングディスパッチ](../api/ene-runtime.md#ストリーミングディスパッチ)を参照）。

- **Mind**（`streaming_cognitive::run_stream_cognitive`）— プロンプト構成、記憶検索、感情処理、ターン後の記憶書き込みを `ene-mind` の `CognitionEngine` に委譲します。

ディスパッチには有効かつ初期化済みのストアと埋め込みプロバイダーが必要です。前提条件が欠ける場合、`EneRuntimeError::MindPrerequisite` を返し、失敗した `Terminal` イベントを発行します。レガシー・ストリームへのフォールバックはありません。

各ターンは [`TurnId`](../api/ene-runtime.md) で識別されます。`run` はその id を返します（既にターン実行中なら `RunError::Busy`）。ターン範囲のチャットイベントは同じ `turn` フィールドを持ちます。`Terminal` は会話履歴のコミットと同期 `finalize_turn`（affect 永続化）の後に発行されます。遅延 LLM 記憶抽出と自然忘却はまだ実行中の場合があります。

## チャット `EneEvent` バリアント

| `EneEvent` バリアント | 補足 |
|---|---|
| `TurnStarted { turn, origin }` | プロバイダーストリーム開始後 |
| `TextDelta { turn, origin, delta }` | プレーンテキスト。感情 / Performance マーカーは除去済み。 |
| `AudioChunk { turn, origin, pcm, sample_rate, is_final }` | `TextDelta` と並行してストリーミングされる TTS 合成音声（TTS プロバイダー設定時のみ）。[音声ストリーミング](#音声ストリーミング)を参照。 |
| `Performance { turn, origin, cues, source }` | Output Arbiter からの提示 cue（`PerformanceCue` / `CueSource` は `ene-mind`）。旧 `SpecialToken` + 単独 `Expression` を置換。 |
| `ToolCallStart` / `ToolCallResult` | ツール実行ライフサイクル（UI が必要な場合）。 |
| `ToolBackgroundCompleted` | 遅延バックグラウンドツール完了（`Terminal` 後でも可）。 |
| `PermissionRequired` / `UserInputRequired` | 対話型ツールのゲート。 |
| `ContextCompressed { turn, origin, level }` | 圧縮実行の薄い信号。詳細は diagnostics。 |
| `Terminal { turn, origin, reason }` | `Run` ごとに正確に1回。履歴コミットと同期 `finalize_turn`（感情永続化）の後。 |
| `StatusChanged { status }` | Idle / Running / Error。 |

外部 JSON は `ene_runtime::PublicChatEvent` /
[`schemas/public-chat-event.v1.json`](../api/schemas/public-chat-event.v1.json) を優先。

### 音声ストリーミング

TTS プロバイダーが設定されている場合（`ai.tts.provider != "none"`）、mind ストリーミングパイプラインは蓄積した `TextDelta` テキストを文単位でプロバイダーへ送り、合成された音声を `AudioChunk` イベントとしてテキストストリームと並行して発行します。

| フィールド | 型 | 説明 |
|-------|------|-------------|
| `turn` | `TurnId` | この音声が属するターン |
| `origin` | `TurnOrigin` | ターンを開始した主体 |
| `pcm` | `Vec<f32>` | `[-1.0, 1.0]` に正規化されたモノラル PCM サンプル |
| `sample_rate` | `u32` | サンプルレート（Hz、例：Kokoro ONNX は `24000`） |
| `is_final` | `bool` | 終端マーカーで `true`（`pcm` は空、`sample_rate = 0`） |

**発行セマンティクス:**

- `is_final = false` のデータチャンクが 0 個以上届き、それぞれが合成 PCM の一部を運びます。チャンクはプロバイダーのネイティブサンプルレートで約 0.25 秒分の音声です。
- `is_final = true`、`pcm = []`、`sample_rate = 0` の終端マーカーが正確に 1 回届きます。これはそのターンの全文がフラッシュされたことを示します。
- TTS が無効、またはプロバイダーの初期化に失敗した場合、`AudioChunk` イベントは発行されません。テキストストリームには影響しません。
- 末尾の文の合成がまだ進行中の場合、`AudioChunk` イベントは `Terminal` の後に届くことがあります。音声再生の終了判定には `Terminal` ではなく `is_final` を使うべきです。

### チャットバスにないもの

API v1 では次はチャット `EneEvent` ではありません。

| 旧 / 診断 | 現在の置き場所 |
|---|---|
| `SpecialToken`、単独 `Expression` | `Performance` に畳み込み（またはテキストから除去） |
| `SessionSplit` | `diagnostics().manual_split()`；任意で薄い `ContextCompressed` |
| `PipelinePhase`、`PipelineMetrics`、`TaskProgress` | `handle.diagnostics().subscribe()` |
| `ToolHealth`、`ProviderHealth`、`ProviderFallback`、`MemoryWrite` | `handle.diagnostics().subscribe()` |
| `Lagged`、`ResyncNeeded` | ブロードキャスト購読者がオーバーフローしたとき (#189)；スナップショットで再同期 |

`DiagnosticEvent::MemoryWrite` は遅延ポストターン記憶抽出が失敗したときに発行されます。失敗は `pending_memory_writes` にエンキューされ（バックオフ付き再試行）、`Terminal` は遅延しません。確認は `/memory pending` / `/memory status`、強制ドレインは `/memory retry`。

## Diagnostics

`handle.diagnostics()` は具象の `EneDiagnostics` ファサードを返します。UI がトレイトを実装する必要はありません。スナップショット、ツール検査、手動圧縮/分割、診断イベントストリームに使います。

## アプリ側コンシューマーのチェックリスト

`ene-cli`（`apps/ene-cli/src/stream.rs`）と `ene-desktop`（`apps/ene-desktop/src/ai_bridge.rs`）は、最小チャットバス（`Performance` と `Terminal` を含む）を既にマッチしています。新しい UI では:

- 一致する `turn` の `Terminal` でターンループを終了すること — `Run` ごとの完了保証はこのシグナルのみ。
- `Performance` cue を VRM / CLI 表示へマップすること。チャットバス上の `SpecialToken` や単独 `Expression` を期待しないこと。
- `ContextCompressed` は任意の薄い信号として扱い、圧縮の詳細が必要なら `manual_split()` / diagnostics を使うこと。
- `AudioChunk` を消費する場合、`pcm` を再生とビゼーム分析へ転送すること。音声の終了検出には `Terminal` ではなく `is_final` を使うこと。

## 関連ドキュメント

- [`ene-runtime` APIリファレンス](../api/ene-runtime.md) — `EneEvent` のフィールドとストリーミングディスパッチ
- [API v1 ADR](../architecture/api-v1.md) — ホスト / イベント契約
- [セッション分割と圧縮](session-split.md) — ハード分割より圧縮を推奨する理由
- [ストリーミングエンジン](streaming.md) — アクター/ハンドルアーキテクチャ
- [認知ランタイムADR](../architecture/cognitive-runtime.md)
