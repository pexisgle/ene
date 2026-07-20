# 会話ストリーミング & アクター制御ループ仕様

本ドキュメントでは、LLMからのテキスト生成、対話型ツールコール、承認ゲート、および認知機能（`ene-mind`）の呼び出しを統制する、会話ストリーミング実行パスの仕様を詳細に定義します。

---

## 1. データ構造

### `PermissionDecision` (公開 / 列挙型)
破壊的アクション（書き込み、削除等）に対するユーザーの承認結果。
*   `AllowOnce`: 今回のみ許可。
*   `AllowSession`: セッション期間中、同じアクション・対象への警告なしで許可。
*   `Deny`: 拒否。ツール実行はキャンセルされエラーが返ります。

### `UserInputResponse` (公開 / 列挙型)
対話型ツールへの応答。
*   `Multi(Vec<MultiAnswer>)`: サブ質問への回答リスト。
*   `Cancel`: ユーザーによるプロンプト全体のキャンセル。

### `StreamContext` (非公開 / 実行コンテキスト)
ストリーミングタスク起動時にアクターから引き渡されるすべての情報。
```rust
pub struct StreamContext {
    pub config: EneConfig,
    pub session: ConversationSession,
    pub user_input: String,
    pub embedder: Option<Arc<dyn EmbeddingProvider>>,
    pub registry: Arc<dyn ToolRegistry>,
    pub tool_rag: Option<Arc<ToolRag>>,
    pub provider: Arc<dyn LlmProvider>,
    pub event_tx: broadcast::Sender<EneEvent>,
    pub diag_tx: broadcast::Sender<DiagnosticEvent>,
    pub cancel_token: CancellationToken,
    pub pending_permissions: Arc<Mutex<HashMap<RequestId, oneshot::Sender<PermissionDecision>>>>,
    pub pending_user_inputs: Arc<Mutex<HashMap<RequestId, oneshot::Sender<UserInputResponse>>>>,
    pub terminal_emitted: Arc<AtomicBool>,
    pub turn: TurnId,
    pub origin: TurnOrigin,
    pub allow_tools: bool,
    pub runtime_directive: Option<String>,
    pub proactive_screen_image: Option<String>,
    pub generation_timeout: Option<Duration>,
    pub classifier_tx: mpsc::UnboundedSender<JoinHandle<()>>,
    pub memory_writer_tx: mpsc::UnboundedSender<JoinHandle<()>>,
}
```

---

## 2. 一般ストリーミング処理 (`streaming.rs`)

`run_stream` は、認知機能（長期記憶、感情モデル）を伴わない最小限の会話ストリームを実行します。

### 主要関数

#### `run_stream`
*   **シグネチャ**: `pub async fn run_stream(ctx: StreamContext) -> StreamOutcome`
*   **制御フロー**:
    1.  Tool RAGまたはレジストリから利用可能ツールをロード (`select_relevant_tools`)。
    2.  `build_messages` を用いてメッセージリストを構築（システム、過去履歴、現在入力を統合）。
    3.  `provider.stream_chat` を呼び出しストリーム取得。
    4.  トークンをパースし、テキストであれば `TextDelta` イベントをブロードキャスト。
    5.  `ToolCall` トークンが検知された場合、チャンクを蓄積し (`accumulate_tool_calls`)、ストリーム一時停止後に `perform_tool_executions` で実行。
    6.  完了時、`stream_finish` を呼び出して `Terminal(Done)` を発行。

#### `perform_tool_executions`
*   **シグネチャ**:
    ```rust
    pub(crate) async fn perform_tool_executions(
        calls: Vec<LlmToolCall>,
        // ... (省略)
    ) -> Result<ToolExecutionOutput, ToolError>
    ```
*   **挙動**:
    -   検出された `LlmToolCall` ごとに、レジストリから定義をロード。
    -   **Sandbox/承認チェック**: ツール仕様に `sandbox = false` などの制限、またはファイルIOなどの破壊的シグナルがあれば、`RequestId` を生成して `PermissionRequired` イベントをUIに送出し、ユーザーの応答を `pending_permissions` 経由で非同期に待ち受けます。
    -   **インタラクティブ入力**: ツールがユーザー追加要求（`UserInputRequired`）を出した場合、同様に `UserInputRequired` を送出し、応答を待ちます。
    -   完了後、ツールプロセスを `call_tool` 経由で実行し、`ToolCallResult` を発行。結果を LLM の再インプットメッセージとして集約。

---

## 3. 認知ランタイムストリーミング処理 (`streaming_cognitive.rs`)

`run_stream_cognitive` は、記憶の回収（Recall）、感情の査定（Appraisal）、長期記憶の自動抽出（Memory Consolidation）を統合した高度なターン実行パイプラインです。

### 実行ライフサイクル (5つのフェーズ)

#### 1. Phase A: 埋め込み生成 & キャラクター同期 (Embedding & Sync)
*   ユーザー入力テキストを `embed_query` によりベクトル化します。
*   セッション情報内のキャラクターカードハッシュ（`ccv3_memory_hash`）とカードの実体ハッシュが異なる場合、SQLiteデータベース内の固有記憶（Lorebook、スタイル記述等）の同期処理 (`engine.sync_character_memories`) を同期的に実行し、ハッシュを更新します。

#### 2. Phase B: 前ターン認知処理 (before_turn)
*   `engine.before_turn` を呼び出し、以下のタスクを並列実行します:
    -   感情状態のロードおよびPAD空間感情モデルの更新。
    -   `MemoryStore` からのハイブリッドメモリリコール（エピソード記憶、セマンティックファクト、ルール、対話スタイルの抽出）。
    -   `ToolRag` によるクエリ適合ツール検索。
*   結果は `PreTurnOutput` として集約されます。

#### 3. Phase C: プロンプトパケット組み立て (compose_prompt_packet)
*   `engine.compose_prompt_packet` を呼び出し、利用可能トークンバジェット制限（`ContextBudget`）を適用した `PromptPacket` を構築します。
*   トークン数が制限を超えている場合は、コグニティブセッションの自動分割・要約処理 (`session_split`) をトリガーします。
*   システムプロンプト、キャラクターパーソナリティ、回想記憶、感情、表情契約（`build_cognitive_output_contract`）、会話履歴、および現在入力を物理的な順序に連結します。

#### 4. Phase D: LLMストリーミング & アクション実行
*   組み立てられたプロンプトを LLM に投入し、チャット接続を開始します。
*   ストリーム受信中に `<|perf:expr=NAME|>` などの表情指示トークンを検知した場合、アバター表情再生用の `Performance` イベントにパース・変換してUIへ送信し、会話テキスト（`TextDelta`）からは除去します。
*   ツールコールが発生した場合は、`perform_tool_executions` を使ってアクター経由で呼び出し、結果を LLM にフィードバックして会話を継続します。

#### 5. Phase E: 後ターン処理 & 永続化 (finalize_turn)
*   ストリーム完了後、`engine.finalize_turn` を呼び出します。
*   今回の会話ログを `conversation_logs` に保存し、最新の感情状態、コミットメントを SQLite に同期的に永続化します。
*   以下の重たい認知ロジックは、ストリーミングレスポンスを遅延させないため、**`Terminal` イベントのブロードキャスト後に非同期（バックグラウンドタスク）として spawn されます**:
    -   **感情分類器 (`spawn_affect_classifier`)**: 今回の対話がマスコットの感情値にどう影響したかをLLM分類器で判定し、次ターンの査定用としてキューに挿入。
    -   **記憶抽出器 (`spawn_memory_writer`)**: 会話履歴からユーザーの好みや重要なエピソードを `MemoryArbiter` を用いて自動抽出し、長期ベクター記憶として SQLite にインデックス化。また、古い記憶の自然減衰（自然忘却モデル）を適用。
