# `EneHandle` & `EneActor` ライフサイクルと通信仕様

本ドキュメントでは、`ene-runtime` のコアアクターロジックである `EneHandle`、`EneActor`、およびアクターへ送信されるコマンド・発行されるイベントの仕様を詳細に定義します。

---

## 1. データ型と列挙型 (Data Structures & Enums)

### `EneCommand` (非公開 / アクターコマンド)
アクターに処理を指示するための `mpsc` チャネル送信用メッセージ。
```rust
pub enum EneCommand {
    Run { input: String, turn: TurnId },
    Cancel { turn: TurnId },
    Shutdown,
    PermissionDecision { request_id: RequestId, decision: PermissionDecision },
    UserInputResponse { request_id: RequestId, response: UserInputResponse },
    GetSnapshot { reply: oneshot::Sender<EneStateSnapshot> },
    ManualSplit { reply: oneshot::Sender<Result<SplitResult, EneRuntimeError>> },
    ListTools { reply: oneshot::Sender<Vec<ToolSpec>> },
    CallTool { name: String, arguments: String, turn: Option<TurnId>, reply: oneshot::Sender<Result<String, EneRuntimeError>> },
    InvalidateToolIndex,
    SetCcv3MemoryHash { hash: u64, reply: oneshot::Sender<()> },
    SetCharacter { card: Box<CharacterCardV3>, reply: oneshot::Sender<Result<(), EneRuntimeError>> },
    UpdateProactiveObservation { observation: ene_mind::ProactiveObservation },
    UpdateProactiveSettings { mind: ene_mind::ProactiveConfig },
    UpdateFeatureSettings { settings: Box<FeatureSettingsUpdate> },
    SummarizeScreenImage { width: u32, height: u32, rgb: Vec<u8>, app_label: String, reply: oneshot::Sender<Result<String, String>> },
}
```

### `EneEvent` (公開 / チャットイベントバス)
`EneHandle::subscribe` で取得されるブロードキャストイベント。すべてのターン関連イベントは `turn: TurnId` および `origin: TurnOrigin` を保持します。
*   `TextDelta { turn, origin, delta }`: 生成されたLLM応答の差分テキスト。表情PHIなどの特殊トークンは除去済み。
*   `Performance { turn, origin, cues, source }`: アバター再生用の表情/動作指示 (`PerformanceCue`)。
*   `ToolCallStart { turn, origin, name, arguments }`: LLMがツール実行を要求した際の通知。
*   `ToolCallResult { turn, origin, name, result }`: ツール実行結果の通知。
*   `PermissionRequired { turn, origin, request_id, action, target, description }`: ファイル書き込みや削除など、破壊的なツール操作時のユーザー承認要求。
*   `UserInputRequired { turn, origin, request_id, prompt }`: clarification question 等の対話型ツールによるユーザー追加入力要求。
*   `ContextCompressed { turn, origin, level }`: 履歴圧縮（セッションスプリット）の実行通知。
*   `Terminal { turn, origin, reason }`: ターン処理の完全終了シグナル。
*   `StatusChanged { status }`: アクターのグローバルステータス変更。
*   `TurnStarted { turn, origin }`: LLMとのコネクション確立およびストリーム開始。

---

## 2. アクター排他制御 (`TurnGate`)

Ene のターン実行は **シングルフライト (Single-flight)** 制約を持ちます。進行中のターンがある状態で新たな `run` を呼び出すと、割り込み処理を行わず、即座に `Busy` エラーを返します。これを atomic かつスレッドセーフに管理するのが `TurnGate` です。

```rust
struct TurnGate {
    busy: AtomicBool,
    active: Mutex<Option<TurnId>>,
}
```
*   `try_begin(&self, turn: &TurnId) -> bool`: `busy` の `compare_exchange` を用いてターンを排他ロックし、ロックに成功した場合は `active` に対象の `TurnId` を記録して `true` を返します。
*   `end(&self)`: アクター側のストリーム完了時に呼び出され、`active` をクリアし、ロックを解除します。
*   `matches(&self, turn: &TurnId) -> bool`: キャンセル対象の `TurnId` が現在アクティブなものと一致するか確認します。

---

## 3. `EneHandle` メソッド仕様

`EneHandle` は、コンシューマーアプリケーションが保持するクローン可能なスレッドセーフハンドルです。

### 主要メソッド

#### `open`
```rust
pub async fn open(config: EneConfig, card: CharacterCardV3) -> Result<Self, EneRuntimeError>
```
*   **初期化シーケンス**:
    1. コマンド送信用 `mpsc` チャネル、イベント/診断ブロードキャストチャネルを生成。
    2. LLMプロバイダーレジストリ（`LlmProviderRegistry`）に factory を登録。
    3. `config` から `MindConfig`, `StoreConfig`, `ToolConfig`, `ToolRagConfig` を抽出・検証。
    4. メモリまたは Tool RAG が有効な場合、`init_embedding` により埋め込みプロバイダーを生成。
    5. コグニティブセッション `ConversationSession` を生成し、キャラクターカードを割り当て、 embedder を紐付け。
    6. `StoreConfig` が有効な場合、`init_memory_store` によりSQLite接続を確率しセッションに紐付け。
    7. `build_tool_registry` により有効化されたツールをスキャン・登録。
    8. Tool RAG が有効な場合、バックグラウンドでのツール仕様インデックス化プロセス（`start_background_indexer`）を開始。
    9. `warmup_character_memories_ready` を実行して、初回起動時のキャラクター固有長期記憶・設定のウォームアップを行い、セッション状態にハッシュを記録。
    10. `EneActor` 構造体を構築し、`tokio::spawn` にてアクターのイベントループタスクを起動。

#### `run`
```rust
pub fn run(&self, input: impl Into<String>) -> Result<TurnId, RunError>
```
*   **挙動**:
    1. 新規 `TurnId` を生成。
    2. `turn_gate.try_begin()` を呼び出し、アクタービジー状態であれば即座に `RunError::Busy` を返却。
    3. `EneCommand::Run` メッセージをチャネル経由でアクターへ送信。

#### `cancel`
```rust
pub fn cancel(&self, turn: &TurnId) -> Result<(), CancelError>
```
*   **挙動**:
    1. `turn_gate.matches()` により、現在進行中のターンと一致するかチェック。不一致であれば `CancelError::TurnMismatch` を返却。
    2. `EneCommand::Cancel` メッセージをアクターへ送信。

#### `shutdown`
```rust
pub async fn shutdown(&self, timeout: Duration) -> Result<(), ShutdownTimeout>
```
*   **挙動**: `EneCommand::Shutdown` を送信し、actorの `JoinHandle` が完了するのを指定の `timeout` 内で待ちます。タイムアウトした場合は `ShutdownTimeout` エラーを返却します。

---

## 4. `EneActor` イベントループ仕様 (Private)

`EneActor::run(mut self)` は `tokio::spawn` された無限ループタスクとして動作し、`cmd_rx` から届くコマンドを順次処理します。

### ターン処理 (`EneCommand::Run`) における主要ライフサイクル
1. **ストリーミングタスクのスポーン**:
   - キャンセルトークン（`CancellationToken`）を用意し、LLM応答をストリームする非同期タスク `streaming::run_stream` を `tokio::spawn` します。
   - アクターの状態を `EneStatus::Running` へ変更し、`StatusChanged` イベントを発行します。
2. **非同期結合タスクの監視**:
   - `streaming::run_stream` は、`TextDelta` を逐次生成しながら、ツール実行要求があれば `CallTool` 等のコマンドをアクター側に戻します。
   - `call_tool_tasks` などの JoinSet を使って、ツール実行非同期タスクの生存期間と結果収集を並行処理ループ内で管理します。
3. **ターミナルの保証**:
   - ターンが成功・失敗・キャンセル（キャンセルトークン発火）のいずれかで終了した際、`finalize_turn` パイプラインを同期的に実行して感情の保存などを行った後、必ず1回だけ `EneEvent::Terminal` をブロードキャストします。
   - ターンの終了時には `turn_gate.end()` が呼び出され、アクターはビジー状態から復帰します。
