# `EneHandle` / アクターメッセージング仕様

`EneHandle` は、Ene アクターシステムと通信するためのメインの非同期スレッドセーフハンドルです。これは、UI イベントループ、デスクトップ統合、または CLI クライアントからの非同期要求を `EneActor` のバックグラウンド実行スレッドに中継します。

---

## 1. 構造体とチャネルの定義

### `EneHandle` (パブリック / 構造体)
```rust
#[derive(Clone)]
pub struct EneHandle {
    sender: mpsc::Sender<ActorCommand>,
}
```

*   `sender`: mpsc 経由でアクターメッセージ要求をスケジュールするための送信メッセージチャネル。

### `EneActor` (プライベート / バックグラウンドアクター)
アクターの実行状態とチャネル接続を監視するプライベートメッセージループ：
```rust
struct EneActor {
    receiver: mpsc::Receiver<ActorCommand>,
    session: ConversationSession,
    mind: CognitionEngine,
    config: EneConfig,
    state: ActorState,
    // その他の非同期実行タスク用の結合ハンドル
}
```

---

## 2. コア通信メソッド (`EneHandle`)

#### `new`
*   **シグネチャ**: `pub fn new(sender: mpsc::Sender<ActorCommand>) -> Self`
*   **説明**: 指定された送信エンドポイントを持つ `EneHandle` インスタンスを作成します。

#### `run`
*   **シグネチャ**: `pub async fn run(&self, input: UserInput, rx: EneEventReceiver) -> Result<TurnReport, EneError>`
*   **プロセス**:
    1.  現在のアクター状態がビジー（既にターンを実行中）でないか検証します。ビジーな場合は `EneError::Busy` を返します。
    2.  `rx` 送信通知ストリームオブジェクトを作成し、チャネル情報をバインドします。
    3.  `ActorCommand::StartTurn` メッセージをアクター送信バッファにポストします。
    4.  アクターからの完了応答（またはエラー情報）を待ち、収集した `TurnReport` を呼び出し元に返します。

#### `stop`
*   **シグネチャ**: `pub async fn stop(&self) -> Result<(), EneError>`
*   **説明**: アクティブな実行ストリームまたはバックグラウンド RAG タスクをただちに強制終了するようアクターにシグナルを送信します。

#### `reset`
*   **シグネチャ**: `pub async fn reset(&self) -> Result<(), EneError>`
*   **説明**: 会話履歴セッション、メモリキャッシュ、および PAD 感情座標を初期状態にリセットします。

---

## 3. アクター実行ループとコマンド処理 (`EneActor`)

`EneActor` は単一のイベントループを処理します。

#### `run_loop`
*   **シグネチャ**: `async fn run_loop(mut self)`
*   **説明**: 受信チャネルのコマンド要求を待ち、メッセージパラメータに応じて対応するプライベートハンドラを実行します。

#### `handle_start_turn`
*   **シグネチャ**: `async fn handle_start_turn(&mut self, input: UserInput, responder: oneshot::Sender<Result<TurnReport, EneError>>, event_tx: EneEventSender)`
*   **プロセス**:
    1.  `TurnGate` 状態をロックし、ビジーモードにします。
    2.  受信した入力テキストを `ConversationSession::add_user_message` 経由で履歴バッファに書き込みます。
    3.  セッション用の非同期実行トークを作成し、`run_stream_cognitive` 処理に制御を移行します。

#### `handle_proactive_turn`
*   **シグネチャ**: `async fn handle_proactive_turn(&mut self, topic: String, responder: oneshot::Sender<Result<TurnReport, EneError>>, event_tx: EneEventSender)`
*   **説明**: プロアクティブ発話要求をトリガーします。会話セッションにユーザー発話を追加する手順をスキップする点を除き、`handle_start_turn` と同様のプロセスに従います。

#### `handle_stop`
*   **シグネチャ**: `async fn handle_stop(&mut self)`
*   **説明**: ストリーミング接続のキャンセルトークンを破棄し、ターンゲートの状態をクリアしてアイドル状態に戻します。

#### `handle_reset`
*   **シグネチャ**: `async fn handle_reset(&mut self)`
*   **説明**: 会話メモリ、進行中のタスク状態、および SQLite メモリデータベース接続の一部を再初期化します。

---

## 4. イベント受信バッファ仕様

### `EneEventReceiver` (パブリック / ストリーム)
アクターとクライアント間のデータパケット受信用の一方向読み取りチャネル。

#### `next`
*   **シグネチャ**: `pub async fn next(&mut self) -> Option<EneEvent>`
*   **説明**: ストリームチャネル内の次の出力を返します（プレーンテキスト用の `TextDelta`、アニメーションキュー用の `Performance`、または `Completed` メッセージ）。

---

## 5. アクター排他ゲート仕様

### `TurnGate` (プライベート / アトミック状態ロック)
アクターが同時に複数の発話要求を処理することを防ぎます。

#### `try_acquire`
*   **シグネチャ**: `pub fn try_acquire(&self) -> Result<GateToken, EneError>`
*   **説明**: 状態をチェックします。アイドル状態の場合のみロックを設定して保護用トークンを返します。

#### `release`
*   **シグネチャ**: `pub fn release(&self, token: GateToken)`
*   **説明**: トークンを解放し、システム状態をアイドル状態に戻します。
