# `CognitionEngine` およびターンライフサイクル仕様

`CognitionEngine` は、`ene-mind` の認知パイプラインの主要なオーケストレータファサードです。ターン前の分析、ハイブリッドメモリの想起、プロンプトのレイアウト構築、表情決定の解決、およびバックグラウンドでのメモリ統合を調整します。

---

## 1. 構造体の定義

### `CognitionEngine` (パブリック / 構造体)
AI パートナーの頭脳を表す、個々のモジュール式サブコンポーネントを集約します：
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

---

## 2. CognitionEngine コアメソッド

#### `new`
*   **シグネチャ**: `pub fn new() -> Self`
*   **説明**: 新しい `CognitionEngine` インスタンスを作成し、各サブコンポーネントを初期状態に設定します。

#### `validate_config`
*   **シグネチャ**: `pub fn validate_config(config: &MindConfig) -> Result<(), CognitionError>`
*   **説明**: `validate_context_config` に検証を委譲し、設定されたトークンサイズとセグメント境界が LLM 制限内に安全に収まるか検証します。

#### `sync_character_memories`
*   **シグネチャ**: `pub async fn sync_character_memories(&self, ctx: TurnContext<'_>, previous_hash: Option<u64>) -> Result<(crate::character::CharacterMemorySyncReport, u64), CognitionError>`
*   **プロセス**:
    1.  バッキングデータベース (`ctx.store`) とベクトル埋め込みプロバイダ (`ctx.embedder`) の存在を確認します。
    2.  `CharacterProcessor::sync_card_memories` を呼び出して、`CharacterCardV3` 内のキャラクター固有のルール、背景設定、および会話スタイルの指示情報を同期します。
    3.  新しいキャラクターカードハッシュを計算し、DB レジストリを更新して同期レポートを返します。

#### `before_turn`
*   **シグネチャ**: `pub async fn before_turn(&self, ctx: TurnContext<'_>) -> Result<PreTurnOutput, CognitionError>`
*   **プロセス**:
    1.  **感情の更新**:
        -   `ctx.store` から感情状態データ (`AffectState` の PAD 座標値) をロードします。
        -   以前のターンから保留されている分類器の提案情報 (`take_pending_affect_proposal`) があれば、ユーザーのターン順序と一致することを確認してポップ・適用します。
        -   前回の更新からの経過時間に基づいて、時間の経過による感情の自然減衰を計算し、`EmotionEngine::update_turn` 経由で新たな感情アプレザル（評価）を実行します。
    2.  **メモリ想起 (Recall)**:
        -   埋め込みプロバイダを検証し、ユーザー入力テキストと対応するベクトルを用いて `execute_hybrid_recall` を実行します。
        -   ベクトルのセマンティック類似度検索、時間的リセンシー、および感情フィルタを組み合わせて、最も関連性の高い過去の記憶コンテキストを選択します。
    3.  **コミットメント (約束) のロード**:
        -   SQLite から `CommitmentLedger::list_active` 経由でアクティブなタスクや約束を最大 16 件取得し、プロンプト事実情報として格納します。
    4.  **アセンブリ**:
        -   これらすべての結果をまとめて、`PreTurnOutput` 構造体を構築して返します。

#### `before_proactive_turn`
*   **シグネチャ**: `pub async fn before_proactive_turn(&self, ctx: TurnContext<'_>) -> Result<PreTurnOutput, CognitionError>`
*   **説明**: キャラクターが自発的に発話する場合（プロアクティブ発話）の軽量なターン前処理。ユーザーの入力テキストが存在しないため、ベクトル計算やハイブリッド想起のプロセスをスキップし、現在の感情状態とアクティブな約束情報のみを読み込みます。

#### `persist_affect_snapshot`
*   **シグネチャ**: `pub async fn persist_affect_snapshot(store: &MemoryStore, affect: &ene_store::AffectState) -> Result<(), CognitionError>`
*   **説明**: 感情の PAD 状態データを SQLite に直接保存し、ストリーム接続のキャンセルやランタイムエラーが発生した場合でも感情座標データを保護します。

#### `compose_prompt_packet`
*   **シグネチャ**: `pub async fn compose_prompt_packet(&self, ctx: TurnContext<'_>, pre: &PreTurnOutput, prefetch: ComposePrefetch) -> Result<ComposedPrompt, CognitionError>`
*   **プロセス**:
    1.  `CharacterProcessor::compile_kernel` を使用して、キャラクターの基本的なパーソナリティテキストをコンパイルします。
    2.  キャラクターカードから会話スタイルの指示情報を読み込んで解決します。
    3.  アクティブな感情パラメータ状態（PAD 座標、気分ラベルなど）をシリアライズします。
    4.  `prefetch` にシーン要約が存在しない場合は、DB から直近の `ActiveSceneSummary` をロードします。
    5.  残りの履歴トークン量を推定し、`pack_prompt` に引き渡します。
    6.  プロンプト全体の推定トークン量が `ContextBudget` の閾値を超えた場合、古い歴史履歴の削除やメモリプロンプトのドロップ処理をトリガーします。
    7.  生成されたプロンプトテキストと予算メタデータを含む `ComposedPrompt` を作成して返します。

#### `after_turn`
*   **シグネチャ**: `pub async fn after_turn(&self, store: &MemoryStore, config: &MindConfig, input: PostTurnInput<'_>, providers: crate::memory_writer::MemoryWriteProviders<'_>) -> Result<(), CognitionError>`
*   **説明**: ターンの終了後に、非同期バックグラウンドで実行するメモリ統合タスク（対話からの新しい事実の抽出、ベクトル化、自然忘却処理など）をディスパッチします。

#### `finalize_turn_post`
*   **シグネチャ**: `pub async fn finalize_turn_post(&self, store: &MemoryStore, config: &MindConfig, input: &PostTurnInput<'_>) -> Result<(), CognitionError>`
*   **説明**: LLM の出力ストリームが完了した直後に同期的に実行されるメソッドです。会話メッセージテキストをログテーブルに保存し、感情座標の更新を SQLite にコミットします。

#### `write_memories_deferred`
*   **シグネチャ**: `pub async fn write_memories_deferred(&self, store: &MemoryStore, config: &MindConfig, input: &crate::lifecycle::OwnedPostTurnInput, providers: crate::memory_writer::MemoryWriteProviders<'_>) -> Result<(), CognitionError>`
*   **プロセス**:
    1.  `MemoryWriter::write_memories` を呼び出し、今回のターンで新たに生じたセマンティックメモリおよびエピソードメモリの候補データを抽出します。
    2.  抽出された候補と既存のデータベース内のメモリを比較し、重複や矛盾がないか仲裁します。
    3.  `MemoryWriter::apply_forgetting` を呼び出し、一定期間アクセスされていない古いアクティブメモリを減衰（Faded）またはアーカイブ（Archived）状態に移行します。

#### `resolve_expression_turn`
*   **シグネチャ**: `pub fn resolve_expression_turn(&self, config: &MindConfig, card: &CharacterCardV3, affect: &ene_store::AffectState, response_text: &str, llm_proposal: Option<&str>, previous_expression: &str, elapsed_since_change: Option<Duration>) -> (crate::output::ExpressionDecision, ene_store::AffectState)`
*   **説明**: キャラクターの表情ブレンドシェイプキーを解決します。現在の感情座標、テキストの句読点、LLM からの表情変更提案、およびヒステリシス保護時間の制約を考慮し、最終的な表情と感情の更新結果を決定して返します。

---

## 3. モジュールレベルのヘルパー関数

#### `build_behavior_contract`
*   **シグネチャ**: `fn build_behavior_contract(card: &CharacterCardV3, user_name: &str) -> Option<String>`
*   **説明**: キャラクターカードから原作者ノートや指示記述を抽出し、`expand_cbs_macros` 経由でプレースホルダーを展開します。

#### `pending_to_affect_proposal`
*   **シグネチャ**: `fn pending_to_affect_proposal(pending: ene_store::PendingAffectProposal) -> crate::emotion::AffectProposal`
*   **説明**: データベースに保存されていた保留中の感情提案データを、アクター処理用の型に変換マッピングします。

#### `count_user_turns`
*   **シグネチャ**: `pub fn count_user_turns(history: &[crate::lifecycle::HistoryEntry]) -> i64`
*   **説明**: 直近の履歴ウィンドウにおけるユーザー発言数を取得します。

#### `completed_user_turn_at_post_turn`
*   **シグネチャ**: `pub fn completed_user_turn_at_post_turn(history: &[crate::lifecycle::HistoryEntry]) -> i64`
*   **説明**: ポストターンにおける感情判定処理用にユーザー発言のインデックスを計算します。

#### `build_classifier_context`
*   **シグネチャ**: `pub fn build_classifier_context(history: &[crate::lifecycle::HistoryEntry], current_assistant: &str, affect: &ene_store::AffectState, max_turns: usize) -> ClassifierContext`
*   **説明**: 会話履歴と最終出力をまとめ、バックグラウンドでの感情分析 LLM プロバイダへ渡すための `ClassifierContext` を生成します。
