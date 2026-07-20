# `CognitionEngine` & ターンライフサイクル仕様

`CognitionEngine` は、`ene-mind` クレートの主たるファサードであり、会話の各ターンで実行される一連の認知処理（事前分析、長期記憶回収、プロンプト生成、表情出力決定、感情・記憶の事後保存および抽出）のオーケストレーションを担います。

---

## 1. 構造体定義

### `CognitionEngine` (公開 / 構造体)
認知機能ごとの各サブコンポーネントをメンバとして集約しています。
```rust
pub struct CognitionEngine {
    pub pre_turn: PreTurnAnalyzer,
    pub context: ContextManager,
    pub memory_writer: MemoryWriter,
    pub recall: RecallPlanner,
    pub emotion: EmotionEngine,
    pub character: CharacterProcessor,
    pub prompt_packet: PromptPacket,
    pub output: OutputArbiter,
    pub commitments: CommitmentLedger,
}
```
*   `new() -> Self`: 各サブコンポーネントをデフォルト値でインスタンス化します。

---

## 2. 構成の検証と同期 (Validation & Sync)

### `validate_config`
*   **シグネチャ**: `pub fn validate_config(config: &MindConfig) -> Result<(), CognitionError>`
*   **解説**: 設定値の中のコンテキスト管理予算（システム、履歴、メモリ回想などの上限トークン値）の合計が安全な範囲に収まっているかを検証します。

### `sync_character_memories`
*   **シグネチャ**:
    ```rust
    pub async fn sync_character_memories(
        &self,
        ctx: TurnContext<'_>,
        previous_hash: Option<u64>,
    ) -> Result<(CharacterMemorySyncReport, u64), CognitionError>
    ```
*   **解説**: キャラクターカードV3に定義された Lorebook（特定単語に反応する固有設定）やスタイル例文、セリフ設定を SQLite に同期保存し、再インデックスします。カードに変化がない（ハッシュが一致する）場合は処理をスキップします。
*   **接続先**: `CharacterProcessor::sync_card_memories`, `MemoryStore`, `EmbeddingProvider`

---

## 3. 前ターン処理 (before_turn / before_proactive_turn)

### `before_turn`
*   **シグネチャ**: `pub async fn before_turn(&self, ctx: TurnContext<'_>) -> Result<PreTurnOutput, CognitionError>`
*   **制御フロー**:
    1.  **感情のロードと更新**:
        -   `MemoryStore` から現在の `AffectState`（PAD感情モデル状態）をロード。
        -   前ターンでバックグラウンド実行された感情分類器の保留査定結果（`PendingAffectProposal`）があれば取得し、今回のターンにブレンド。
        -   前回の発話からの経過時間（時間減衰）とユーザーの現在入力を考慮し、`EmotionEngine` で一次感情を更新。
    2.  **長期記憶回収**:
        -   ユーザー入力のベクトル（埋め込み）を用いて、`execute_hybrid_recall` を実行。直近の話題や感情状態に応じた関連エピソード・キーファクトを回想記憶として取得。
    3.  **コミットメントロード**:
        -   キャラクターが保持している未完了タスク（約束・約束履歴）を `CommitmentLedger::list_active` から最大16件抽出し、システム用候補として形式化。
    4.  **出力**:
        -   感情情報、回想記憶、コミットメント、感情分類器による推奨表情をまとめた `PreTurnOutput` を返却。

### `before_proactive_turn`
*   **シグネチャ**: `pub async fn before_proactive_turn(&self, ctx: TurnContext<'_>) -> Result<PreTurnOutput, CognitionError>`
*   **解説**: 能動発話用の軽量版 `before_turn`。ユーザーからのクエリ入力が存在しないため、ベクトル生成や長期記憶回収処理をすべてスキップし、感情のロードとコミットメントのロードのみを高速に実行します。

---

## 4. プロンプトパケット組み立て (compose_prompt_packet)

### `compose_prompt_packet`
*   **シグネチャ**:
    ```rust
    pub async fn compose_prompt_packet(
        &self,
        ctx: TurnContext<'_>,
        pre: &PreTurnOutput,
        prefetch: ComposePrefetch,
    ) -> Result<ComposedPrompt, CognitionError>
    ```
*   **組み立て手順**:
    1.  `CharacterProcessor::compile_kernel` を呼び出し、キャラクターの自己認識コア（Identity Kernel）をコンパイル。
    2.  ユーザー入力に関連する対話スタイル例文（Style Examples）を取得。
    3.  感情状態（PAD値と現在のムードラベル）をフォーマット。
    4.  `load_active_scene_summary` により、現在のシーン状況テキストをロード。
    5.  履歴の切り出し（`recent_turns` 設定値に基づく件数制限）。
    6.  これらのデータを `PackInput` 構造体にまとめ、設定された `ContextBudget`（トークン予算）に従って `pack_prompt` を実行。予算が逼迫したセクションは自動的にトリミングまたは圧縮されます。
    7.  生成された LLM メッセージリストとメタ情報を `ComposedPrompt` として返却。

---

## 5. 後ターン処理 (finalize_turn_post / write_memories_deferred)

### `finalize_turn_post` (同期最終処理)
*   **シグネチャ**: `pub async fn finalize_turn_post(&self, store: &MemoryStore, config: &MindConfig, input: &PostTurnInput<'_>) -> Result<(), CognitionError>`
*   **解説**: LLMの出力が完了した直後に**同期的**に実行されます。更新された最新の `AffectState`（PAD感情）のみをデータベースにupsertし、ユーザーへのレスポンス速度を最優先します。

### `write_memories_deferred` (遅延書き込み処理)
*   **シグネチャ**:
    ```rust
    pub async fn write_memories_deferred(
        &self,
        store: &MemoryStore,
        config: &MindConfig,
        input: &OwnedPostTurnInput,
        providers: MemoryWriteProviders<'_>,
    ) -> Result<(), CognitionError>
    ```
*   **解説**: ターン応答のブロードキャスト後にバックグラウンドで実行されます。
    1.  `MemoryWriter::write_memories`: 今回の会話履歴から `MemoryArbiter`（LLMによる重要エピソードおよびファクト抽出器）を起動し、長期ベクター記憶を抽出して永続化。
    2.  `MemoryWriter::apply_forgetting`: 保存された長期ベクター記憶に対して、自然忘却（時間の経過による想起スコアの減衰、および一定以下になった記憶のステータス遷移）を適用。

---

## 6. 表情出力決定 (resolve_expression_turn)

### `resolve_expression_turn`
*   **シグネチャ**:
    ```rust
    pub fn resolve_expression_turn(
        &self,
        config: &MindConfig,
        card: &CharacterCardV3,
        affect: &ene_store::AffectState,
        response_text: &str,
        llm_proposal: Option<&str>,
        previous_expression: &str,
        elapsed_since_change: Option<Duration>,
    ) -> (ExpressionDecision, ene_store::AffectState)
    ```
*   **解説**: 今回のターンのアシスタント発話に対する、マスコットの最終的な 3D 表情出力を決定します。
*   **制御ロジック**: `OutputArbiter::resolve` を呼び出し、現在のPAD感情値、会話テキスト（感嘆符や疑問符の有無）、直前の表情からの経過時間（チャタリング/激しい表情変化を防ぐためのヒステリシス制御）を総合的に考慮して、最適な表情を判定します。判定された表情名を反映した `AffectState` と意思決定情報 `ExpressionDecision` を返却します。
