# API v1 ホスト契約

`ene-runtime` は組み込みアプリ（`ene-cli`・`ene-desktop`）や外部クライアントに
安定したバージョン付き契約を公開します。契約は
`ene_runtime::public_api` に定義され、意図的に小さく保たれています。

## バージョニング

`API_VERSION = "1"`。

バージョンアップが必要な場合:

- `Public*` 型（またはそのフィールド）の削除・リネーム、
- 既存フィールドの意味やワイヤ形状の変更、
- `PublicApiError` バリアントの削除・形状変更、
- 契約メソッドのシグネチャ変更。

不要な場合:

- 新しい `Public*` 型・バリアント・任意フィールドの追加
  （`PublicApiError` は `#[non_exhaustive]`）、
- 内部エラー enum のバリアント追加（`From` 実装で `PublicApiError` に
  射影される）、
- ホスト内部の `EneHandle` メソッド変更（契約外と明示）。

## 契約に含まれるもの

### Public 型

| 型 | 意味 |
|---|---|
| `PublicChatEvent` | チャットバスの JSON ミラー。`type` タグは snake_case（`turn_started`・`text_delta`・`tool_call_start`・`tool_call_result`・`permission_required`・`user_input_required`・`context_compressed`・`terminal`・`performance`・`beat_pulse`） |
| `PublicLifecycleEvent` | ライフサイクルバスの JSON ミラー（`status_changed`・`pending_candidates_available`・`candidate_changed`・`memory_ledger_changed`・`tool_background_completed`・`connector_changed`） |
| `PublicSessionMeta` | セッション一覧のメタデータ |
| `PublicExportedMessage` | リダクション済み会話メッセージ 1 件 |
| `PublicPerfCue` | パフォーマンスキュー 1 件（`expression`/`motion`/`lookat`/`cancel`、source `affect`/`llm`） |
| `PublicApiError` | 安定エラーカテゴリ: `actor_dead`・`not_found`・`storage`・`invalid`・`internal` |
| `redact_text` / `redact_tool_arguments*` | リダクションヘルパー |

どの `Public*` フィールドにも `ene_store`/`ene_mind`/`ene_plugin_proto` 型は
現れません（コンパイル時テストで強制）。

### 契約メソッド

契約に含まれる `EneHandle` メソッドはこれだけです（シグネチャは
`Public*` 型とプリミティブのみ）:

| メソッド | 目的 |
|---|---|
| `list_sessions` | セッション一覧（新しい順） |
| `export_session` | セッションをバージョン付き・リダクション済み JSON バンドルにエクスポート |
| `import_session` | バンドルをインポート |
| `search_sessions` | メッセージを検索 |
| `archive_session` | セッションのアーカイブ/解除 |

`EneHandle` の他のすべて（`run`・`subscribe`・`take_audio_stream`・
`diagnostics`・権限/undo/機能メソッド・読み取り専用ハンドル）はホスト内部の
配線であり、バージョンアップなしで変更されることがあります。

## 3 チャネルのイベントバス

```text
チャットバス     EneEvent            ブロードキャスト  subscribe()
ライフサイクルバス LifecycleEvent     ブロードキャスト  subscribe_lifecycle()
音声チャネル     AudioChunk          mpsc             take_audio_stream()（単一コンシューマ）
```

チャット/ライフサイクルバスには JSON ミラーがあります。音声チャネルには
ありません（重いプロセス内ストリーミングパスのため）。

## エラー射影

内部エラーは `PublicApiError` カテゴリにマップされます:

| 内部失敗 | カテゴリ |
|---|---|
| DB/バックアップ/マイグレーション/スキーマ問題 | `storage` |
| 呼び出し元入力の不正（埋め込み・遷移・編集・形式） | `invalid` |
| "not found" 形状のストアエラー | `not_found` |
| その他（将来の新バリアント含む） | `internal` |
| アクタータスクの停止 | `actor_dead`（アクター制御・診断・ビジョン・ツールハンドルで統一） |

`run`/`cancel` は専用エラー型（`RunError` の `Busy`・`CancelError` の
`TurnMismatch`）を維持します。呼び出し側がこれらのバリアントで分岐するため
です。

## リダクション

すべてのイベントはシリアライズ前にリダクション境界を通過します。ツール引数
は機微キーが削られ、自由文イベントはシークレットパターンのリダクション
（API キー・Bearer トークン・PEM）を通り、エクスポートセッションはストア層
でリダクションされます。

## 契約の組み込み

正規の例は `crates/ene-runtime/examples/minimal_chat.rs`
（ホストブートストラップ・ターン実行・イベント消費）です。`EneHandle::open`
がブートストラップを行い、`open_from_disk` / `open_with_config` がアプリの
使う設定駆動ヘルパーです。
