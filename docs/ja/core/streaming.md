# ストリーミングエンジン

ene は **アクターベースのメッセージパッシングアーキテクチャ** を使用して、ツール呼び出しループ付きのストリーミング LLM 対話を実行します。

## アーキテクチャ

```
コンシューマー (CLI/デスクトップ)
    ↓ EneCommand::Run { input }
EneHandle (mpsc チャンネル)
    ↓
EneActor (バックグラウンド tokio タスク)
    ├── 所有: セッション, 設定, ツールレジストリ, 権限
    ├── 生成: ストリームタスク (run_stream)
    │     ↓ EneEvent (broadcast チャンネル)
    └── コンシューマーがイベントを受信
```

## EneHandle

コンシューマー向けの公開 API。スレッドセーフでクローン可能。

```rust
pub struct EneHandle {
    cmd_tx: Arc<mpsc::UnboundedSender<EneCommand>>,
    event_tx: broadcast::Sender<EneEvent>,
}
```

### 主要メソッド

| メソッド | 説明 |
|---------|------|
| `new()` | アクターをバックグラウンドタスクとして生成 |
| `run(input)` | `EneCommand::Run` を送信 (ファイア＆フォーゲット) |
| `cancel()` | `EneCommand::Cancel` を送信 |
| `decide_permission(request_id, decision)` | 破壊的操作への許可決定を送信 |
| `submit_user_input(request_id, response)` | ユーザー対話のために一時停止するツールへのユーザー入力応答を送信 |
| `subscribe()` | イベント用の新規 `EneEventReceiver` を取得 |
| `get_snapshot()` | oneshot 経由で読み取り専用状態をリクエスト |
| `load_character(name)` | 名前またはパスからキャラクターカードを読み込み |
| `load_config()` | デフォルトパスから設定を読み込み適用 |
| `load_config_from(assets_dir, config_path)` | 指定パスから設定を読み込み適用 |
| `reconfigure(config)` | oneshot 経由で新しい設定を適用 |
| `manual_split()` | oneshot 経由でセッション分割をトリガー |
| `list_tools()` | oneshot 経由で登録済みツールスペック一覧を取得 |
| `call_tool(name, arguments)` | oneshot 経由でツールを直接呼び出し |
| `invalidate_tool_index()` | Tool RAG のキャッシュ済みツール埋め込みを無効化 |

### EneEventReceiver

`EneHandle::subscribe()` で取得。broadcast 受信機に ergonomic なポーリングメソッドを提供。

| メソッド | 説明 |
|---------|------|
| `try_recv()` | ノンブロッキングポーリング (Bevy ECS 向け) |
| `recv()` | 非同期受信 (tokio タスク向け) |

### ライフサイクル

- `EneHandle::new()` がアクターを生成しハンドルを返す
- クローンは新しい broadcast 受信機を作成（`run()` 前にクローンすればイベントロストなし）
- `Drop`: `Arc::strong_count == 1` のときのみ `Shutdown` を送信（最後のハンドル）
- アクターは `cmd_rx` が `None` を返すと終了（全送信者がドロップされたとき）

## EneCommand

コンシューマーからアクターに送信されるコマンド:

```rust
pub enum EneCommand {
    Run { input: String },
    Cancel,
    Shutdown,
    Reconfigure { config: EneConfig, reply: oneshot::Sender<Result<(), EneCoreError>> },
    LoadCharacter { path: String, reply: oneshot::Sender<Result<(), EneCoreError>> },
    PermissionDecision { request_id: RequestId, decision: PermissionDecision },
    UserInputResponse { request_id: RequestId, response: UserInputResponse },
    GetSnapshot { reply: oneshot::Sender<EneStateSnapshot> },
    ManualSplit { reply: oneshot::Sender<Result<SplitResult, EneCoreError>> },
    ListTools { reply: oneshot::Sender<Vec<ToolSpec>> },
    CallTool { name: String, arguments: String, reply: oneshot::Sender<Result<String, ToolError>> },
    InvalidateToolIndex,
}
```

## EneEvent

アクターから全コンシューマーに broadcast チャンネルで送出されるイベント:

```rust
pub enum EneEvent {
    TextDelta { delta: String },
    SpecialToken { token: String },
    ToolCallStart { name: String, arguments: String },
    ToolCallResult { name: String, result: String },
    PermissionRequired { request_id: RequestId, action: String, target: String, description: String },
    UserInputRequired { request_id: RequestId, prompt: UserInputPrompt },
    TaskProgress { task_id: String, step: usize, total_steps: Option<usize>, description: String },
    SessionSplit { summary: String, reason: SplitReason },
    Done,
    Failed { message: String },
    StatusChanged { status: EneStatus },
}
```

**注意:** `TextDelta` はプレーンテキストのみを含みます。特殊トークン（`<|emo:name|>` など）は `ene-core` 内のストリームタスクで事前にパースされ、別々の `SpecialToken` イベントとして送出されます。

## 内部ストリームフロー (`run_stream`)

アクターは各 `Run` コマンドに対してストリームタスクを生成:

```
Run { input }
  ↓
1. ペンディング分割を適用 (ある場合)
2. 分割条件をチェック → 分割タスクを生成 (必要な場合)
3. 入力を埋め込み → pending_embedding
4. セッションにユーザー入力を記録
5. LLM プロバイダを作成
6. ストリームタスクを生成
  ↓
ストリームタスク (run_stream):
  ├── 記憶コンテキストを取得 (要約 + キーファクト)
  ├── メッセージを構築 (システムプロンプト, 履歴, 記憶, プロトコル)
  ├── 関連ツールを選択 (Tool RAG)
  ├── メインループ (最大 max_tool_call_rounds 回):
  │     ├── LLM ストリーミング → TextDelta / SpecialToken イベント
  │     ├── tool_calls がある場合:
  │     │     ├── ToolCallStart イベント
  │     │     ├── ツール実行 (権限チェック / ユーザー入力の必要な場合あり)
  │     │     ├── ToolCallResult イベント
  │     │     └── ループ継続
  │     └── tool_calls がない場合:
  │           ├── アシスタントログを保存
  │           └── Done イベント
  └── 更新されたセッションを oneshot でアクターに送信
```

## 権限処理

破壊的なツール操作にはユーザー承認が必要:

```
ツール実行 → PermissionRequired { request_id, action, target, description }
  ↓
アクターが PermissionRequired イベントをコンシューマーに送信
  ↓
コンシューマーが権限ダイアログを表示
  ↓
コンシューマーが EneCommand::PermissionDecision { request_id, decision } を送信
  ↓
アクターが pending_permissions マップを介して待機中のストリームタスクに決定をルーティング
  ↓
ストリームタスクがツール実行を再開または拒否
```

権限はアクターとストリームタスク間の `Arc<Mutex<HashMap<RequestId, oneshot::Sender<PermissionDecision>>>>` を介して解決されます。

## ユーザー入力処理

一部のツール（例: `utility.question`）は実行を一時停止し、ユーザーに対話入力を求めます:

```
ツール実行 → ToolError::UserInputRequired { request_id, prompt }
  ↓
ストリームタスクが EneEvent::UserInputRequired { request_id, prompt } を送出
  ↓
コンシューマーが対話ダイアログを表示（選択可能なオプション + フリーテキスト）
  ↓
コンシューマーが EneCommand::UserInputResponse { request_id, response } を送信
  ↓
アクターが pending_user_inputs マップを介して待機中のストリームタスクに応答をルーティング
  ↓
ストリームタスクが _user_answers をツール引数に注入し、ツールを再呼び出し
```

`UserInputResponse` は `Multi(Vec<MultiAnswer>)`（質問ごとに 1 つの回答）または `Cancel` です。各 `MultiAnswer` は `Selected { option }`、`Answer { text }`、または `Skip` です。

## セッション更新

ストリームタスクが完了すると、更新された `ConversationSession` を oneshot チャンネルでアクターに送信します。アクターはこの完了をポーリング:

- **ストリーミング中:** `tokio::select!` と 100ms スリープで `stream_session_rx` をチェック
- **アイドル時:** `cmd_rx.recv()` でブロック（タイマーポーリングなし）
- 完了時: アクターが `self.session` を更新し `StatusChanged { status: Idle }` を送出

## キャンセル

`EneCommand::Cancel` は以下をトリガー:
1. `cancel_token.cancel()` — LLM ストリーミングループ内でチェック
2. `stream_handle.abort()` — tokio タスクをキル
3. セッション状態をアイドルにリセット

キャンセルトークンは `while let Some(chunk) = stream.next().await` の内側でチェックされ、即座に応答します。

## エラーハンドリング

| エラーソース | 処理 |
|-------------|------|
| LLM API エラー | `EneEvent::Failed` + `Done`、ストリームが返す |
| ツールタイムアウト (60秒) | ツールエラーメッセージを LLM に送信 |
| 権限拒否 | ツールエラーを LLM に送信 |
| ユーザー入力キャンセル | ツールエラーを LLM に送信 |
| 最大ラウンド超過 | `EneEvent::Failed` + `Done` |
| 埋め込みエラー | `EneEvent::Failed` + `Done` |
| Broadcast Lagged | コンシューマーが警告をログし、残りのイベントを継続読み込み |

## ツール呼び出しの蓄積

ストリーミング中のツール呼び出しはチャンク単位で到着し、蓄積する必要があります:

```rust
fn accumulate_tool_calls(chunks: &mut Vec<ToolCallChunk>, delta: &[ToolCallChunk])
fn finalize_tool_calls(chunks: Vec<ToolCallChunk>) -> Vec<ToolCall>
```

各チャンクは `index` フィールドで識別されます。`function.arguments` 文字列はチャンク間で連結されます。

## スクリーンショット処理

ツール結果が `{"type":"screenshot","data":"data:image/png;base64,..."}` 形式の場合、base64 データが抽出され、画像 URL を含む `LlmMessage::User` に変換されて次の LLM API 呼び出しに送られます。
