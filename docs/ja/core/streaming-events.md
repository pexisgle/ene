# ストリーミングイベント：レガシー vs 認知（Cognitive）

`ene-core` のアクターは、すべての `EneCommand::Run` を2つのストリーミングパイプライン実装のいずれかにディスパッチします（[`ene-core` APIリファレンス § ストリーミングディスパッチ](../api/ene-core.md#ストリーミングディスパッチ)を参照）。

- **レガシー**（`run_stream_legacy`、`streaming.rs` 内）— 埋め込み → 記憶検索 → メッセージ構築 → ストリーミングという元来のループ。
- **認知（Cognitive）**（`streaming_cognitive::run_stream_cognitive`）— プロンプト構成、記憶検索、感情処理、ターン後の記憶書き込みを `ene-cognition` の `CognitionEngine` に委譲します。

ディスパッチ条件は `cognition.enabled && memory.enabled && embedder.is_some()` です。認知と記憶が両方有効でも埋め込みプロバイダーが未設定の場合、そのターンは黙ってレガシーにフォールバックします。両パイプラインとも同じチャネル上で [`EneEvent`](../api/ene-core.md#eneevent) をブロードキャストするため、**コンシューマー側はどちらのパイプラインが処理したかを知る必要はありません** — ただし、どのバリアントが実際に発生し得るかという「集合」は両パスで異なります。本ページはその差異を明確にするために作成されました（[APIリファクタリング計画](../architecture/api-refactor-plan.md)の項目4を参照）。

## パス別バリアント対応表

| `EneEvent` バリアント | レガシー | 認知 | 補足 |
|---|---|---|---|
| `TextDelta` | ✅ | ✅ | LLMストリームからのプレーンテキストチャンク。 |
| `SpecialToken` | ✅ | ✅（条件付き） | 生のモデル出力に `<\|emo:name\|>` トークンが含まれている限り発行されます。認知ディスパッチ下では、ストリーム中のトークンが *抑制*（`SpecialToken` として送信されない）されるのは `cognition.emotion.enabled && cognition.emotion.llm_expression_is_advisory` の場合のみです — つまり感情エンジンが有効かつLLM提案を助言的として扱うモードでは、生トークンを表に出す代わりにエンジン自身が `Expression` を解決します。`emotion.enabled == false` の場合、または助言モードが無効な場合は、レガシーパスと全く同様にトークンがそのまま `SpecialToken` としてストリームされます。 |
| `Expression` | ❌ | ✅（条件付き） | 認知ランタイムの Output Arbiter（#91）によって、ツール呼び出しの残っていないターンの終了時に解決されるエンジン管理の表情。`cognition.emotion.enabled == true` の場合のみ発行されます。レガシーパスでは発行されず、レガシーはインラインの `<\|emo:name\|>` トークン（`SpecialToken`）のみに依存します。 |
| `ToolCallStart` / `ToolCallResult` | ✅ | ✅ | どちらのパスも同じ共有ツール実行機構（`select_relevant_tools`、`perform_tool_executions`、`accumulate_tool_calls`、`finalize_tool_calls`）を呼び出すため、ツール呼び出しイベントは両パスで同一です。 |
| `PermissionRequired` / `UserInputRequired` | ✅ | ✅ | 上記と同様、共有の `perform_tool_executions` が発行元です。 |
| `TaskProgress` | ✅ | ✅ | 長時間実行のツール呼び出しからどちらのパスでも転送されます。パイプラインに固有のものではありません。 |
| `PipelinePhase` | ✅ | ✅ | 生成前フェーズ（`Embedding`、`Context Search`、`Prompt Building`）への遷移を示します。両パイプラインとも発行しますが、認知パスのフェーズはレガシーの記憶検索/メッセージ構築ステップではなく `CognitionEngine::before_turn`/`compose_prompt_packet` に対応します。 |
| `PipelineMetrics` | ✅ | ❌ | 現状レガシー専用です。最初の `TextDelta` の直前に一度だけ、各フェーズの経過ミリ秒とともに発行されます。認知パスには現時点で同等のメトリクススナップショットがなく、認知パイプラインのフェーズ別タイミングが重要になった場合は埋めるべきギャップです。 |
| `SessionSplit` | ✅（アクターレベル） | ⚠️（下記参照） | どちらのストリーミングパイプラインからも直接発行されません — アクターの自動分割チェック（`apply_pending_split`）または `EneHandle::manual_split()` から発行され、これらは直前のターンをどちらのパイプラインが処理したかに関係なく独立して動作します。 |
| `Terminal` | ✅ | ✅ | 両パス共有の `emit_terminal` + `terminal_emitted` ガードにより、`Run` ごとに正確に1回発行されることが保証されています。 |
| `StatusChanged` | ✅（アクターレベル） | ✅（アクターレベル） | どちらのストリーミング関数でもなく、ディスパッチ前後にアクター自身が発行します。 |

## `SessionSplit` / 圧縮のギャップ

`EneHandle::manual_split()` およびアクターの自動分割チェックは、いずれも `handle_manual_split` を呼び出し、`cognition.enabled && cognition.context.compression_enabled` によって分岐します。

- **レガシー分岐**（圧縮無効、または認知無効時）：`ene_session::execute_split` を実行し、分割を適用して `EneEvent::SessionSplit { summary, reason }` をブロードキャストします。
- **圧縮分岐**（`handle_manual_compression`）：`execute_compression` を実行して履歴を刈り込み、`SplitResult` を呼び出し元に返します — **しかし `EneEvent` は一切ブロードキャストしません**。イベントストリームのみを監視するコンシューマー（`manual_split()` の戻り値をポーリングしない場合）は、現時点では圧縮パスが実行されたことを観測する手段がありません。

これは [APIリファクタリング計画](../architecture/api-refactor-plan.md) の項目4「フェーズB」で追跡されている既知のギャップです。優先度が上がった際には、圧縮指向のイベント発行（または `SessionSplit` の拡張）が計画されています。それまでは、`SessionSplit` は「**レガシー形式のハード分割**が発生した」ことを示すものとして解釈すべきです — このイベントが発行されないことは、セッション履歴に何も起きていないことを意味しません。

## アプリ側コンシューマーのチェックリスト

`ene-cli`（`apps/ene-cli/src/stream.rs`）と `ene-desktop`（`apps/ene-desktop/src/ai_bridge.rs`）は、いずれも現行のすべての `EneEvent` バリアント（`Terminal` と `Expression` を含む）を既にマッチしています — これは本ドキュメントを作成したAPIリファクタリング作業の一環として再確認済みです。新しいUIを実装する際はこれら2つのいずれかをリファレンスとしてイベントループを構築し、以下を守ってください。

- どの単一の「成功」バリアントでもなく、必ず `Terminal` でループを終了すること — `Run` ごとに1回だけ発行が保証されているのはこれだけです。
- `SpecialToken` から派生する感情トークンを扱っていても、`Expression` も必ず処理すること。認知と助言的感情モードが有効なキャラクターは `Expression` のみを送信し、感情系の `SpecialToken` は一切送信しません。
- `SessionSplit` は上記のギャップの通りレガシーパス専用の信号として扱い、コンテキスト管理パスのたびに必ず発火するという前提でUXを構築しないこと。

## 関連ドキュメント

- [`ene-core` APIリファレンス](../api/ene-core.md) — `EneEvent` の全フィールドリファレンスとストリーミングディスパッチ条件
- [セッション分割と圧縮](session-split.md) — ハード分割よりも圧縮が推奨される理由
- [ストリーミングエンジン](streaming.md) — アクター/ハンドルアーキテクチャ（レガシーパス中心の記述で、認知ディスパッチや `Expression`/`Terminal` バリアントが追加される以前の内容）
- [認知ランタイムADR](../architecture/cognitive-runtime.md)
- [APIリファクタリング計画](../architecture/api-refactor-plan.md) — 項目4（イベント/セッション周りの移行）
