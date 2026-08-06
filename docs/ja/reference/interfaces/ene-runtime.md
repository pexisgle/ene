# `ene-runtime` インターフェース

## 役割

アクターベースのホストファサード。`EneHandle` が組み込みアプリの入口です。
キャラクターを開き、ターンを実行し、イベントを消費します。mind・store・AI・
プラグインホスト・RAG・コネクタ・スケジュール・undo を合成します。

## 公開モジュール

| モジュール | 内容 |
|---|---|
| `handle` | `EneHandle`・`EneEvent`・`EneEventReceiver`・`LifecycleEvent`・`LifecycleReceiver`・`AudioChunk`・`AudioStreamReceiver`・`EneStatus`・`EneStateSnapshot`・`ProviderCatalog`・`TerminalReason`・`DeferredToolTask`・`FeatureSettingsUpdate`・`MemoryLedgerChange`・`ShutdownTimeout` |
| `public_api` | `API_VERSION`・`PublicChatEvent`・`PublicLifecycleEvent`・`PublicSessionMeta`・`PublicExportedMessage`・`PublicPerfCue`・`PublicApiError`・リダクションヘルパー |
| `types` | `TurnId`・`TurnOrigin`・`RunError`・`CancelError`・`RequestId` |
| `query` | `MemoryCandidateHandle`・`MemoryLedgerHandle`・`SessionQueryHandle` |
| `tools` | `ToolHandle` |
| `workspace` | `WorkspaceHandle`・`WorkspaceIndexer`・`WorkspaceStatusView` |
| `vision` | `VisionHandle` |
| `connectors` | `ConnectorHandle`・`ConnectorHandleError` |
| `diagnostics` | `EneDiagnostics`・`DiagnosticEvent`・`MemoryHandle` |
| `undo` | `UndoReport` |
| `bootstrap` | `EneHandle::open`・`open_from_disk`・`open_with_config`・`open_ready` |
| `error` | `EneRuntimeError` |
| `task_config` | `ToolRuntimeConfig`（有界タスク受付上限） |
| hidden | `streaming`（権限/ストリーム内部）・`message_builder`・`scheduler`・`proactive*` — `#[doc(hidden)]`。契約外 |

## 主要な `EneHandle` 面

- 開閉: `open(config, card)`・`shutdown(timeout)`。
- ターン: `run(input) -> TurnId`・`cancel(&TurnId)`・`active_turn()`。
- イベント: `subscribe()`（チャット）・`subscribe_lifecycle()`・
  `take_audio_stream()`。
- キャラクター/セッション: `set_character`・`set_greeting`・`card_name`・
  `session_id`・`turn_count`・`history`・`compress_context`。
- 権限/ツール: `decide_permission`・`submit_user_input`・
  `list_permissions`・`revoke_permission`・`reset_all_permissions`・`undo`・
  `tools()`・`provider_catalog()`。
- スケジュール: `add_schedule`・`list_schedules`・`list_schedule_runs`・
  `delete_schedule`・`set_schedule_enabled`。
- 読み取り専用ハンドル: `sessions()`・`candidates()`・`memory_ledger()`・
  `vision()`・`workspace()`・`connectors()`・`diagnostics()`。

## 依存関係

- 依存: `ene-mind`・`ene-store`・`ene-ai`・`ene-rag`（`tool` 付き）・
  `ene-plugin-host`・`ene-config`・`ene-connector`・`ene-core`。
- 利用: `ene-cli`・`ene-desktop`・外部組み込みアプリ。

## リファクタリングの注目点

- **API v1 は正確に** `Public*` 型 + 5 つのセッションメソッド
  （`list_sessions`・`export_session`・`import_session`・`search_sessions`・
  `archive_session`）です。`EneHandle` の残りはホスト内部の配線であり、
  自由に変更できます（[API v1](../architecture/api-v1.md)参照）。
- アクターモデルは要です。コマンドとバックグラウンドタスクは
  `catch_unwind` で隔離され、リリースプロファイルは `panic = "unwind"` を
  維持します。隔離をリファクタリングで壊さないでください。
- 3 チャネルのイベントバス（チャット/ライフサイクル/音声）はトラフィック
  種別の相互枯渇を防ぐためのものです。イベント再構成時も分割を維持して
  ください。
- 他クレートからの再エクスポート（`EneConfig`・`LlmMessage`・
  `CharacterCardV3` など）は便利さのためであり契約ではありません。新 API は
  所有クレートを優先してください。
