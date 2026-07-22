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
| `Performance { turn, origin, cues, source }` | Output Arbiter からの提示 cue（`PerformanceCue` / `CueSource` は `ene-mind`）。旧 `SpecialToken` + 単独 `Expression` を置換。 |
| `ToolCallStart` / `ToolCallResult` | ツール実行ライフサイクル（UI が必要な場合）。 |
| `ToolBackgroundCompleted` | 遅延バックグラウンドツール完了（`Terminal` 後でも可）。 |
| `PermissionRequired` / `UserInputRequired` | 対話型ツールのゲート。 |
| `ContextCompressed { turn, origin, level }` | 圧縮実行の薄い信号。詳細は diagnostics。 |
| `Terminal { turn, origin, reason }` | `Run` ごとに正確に1回。履歴コミットと同期 `finalize_turn`（感情永続化）の後。 |
| `StatusChanged { status }` | Idle / Running / Error。 |

外部 JSON は `ene_runtime::PublicChatEvent` /
[`schemas/public-chat-event.v1.json`](../api/schemas/public-chat-event.v1.json) を優先。

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

## 関連ドキュメント

- [`ene-runtime` APIリファレンス](../api/ene-runtime.md) — `EneEvent` のフィールドとストリーミングディスパッチ
- [API v1 ADR](../architecture/api-v1.md) — ホスト / イベント契約
- [セッション分割と圧縮](session-split.md) — ハード分割より圧縮を推奨する理由
- [ストリーミングエンジン](streaming.md) — アクター/ハンドルアーキテクチャ
- [認知ランタイムADR](../architecture/cognitive-runtime.md)
