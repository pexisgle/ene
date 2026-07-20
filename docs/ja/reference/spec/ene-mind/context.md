# `ContextManager` / セッション圧縮およびトークン予算仕様

このドキュメントでは、Ene のコンテキストトークン制限の予算管理（Context Budget）、優先度ベースのプロンプト構成（Prompt Packing）、およびスライド窓式履歴圧縮プロセスについて詳細に定義します。

---

## 1. 決定論的トークン見積もり (`tokens.rs`)

リアルタイムチャットストリーミング中のオーバーヘッドを避けるため、Ene は外部トークナイザを使用せず、文字ベースの見積もりルールを採用しています。

#### `estimate_tokens`
*   **シグネチャ**: `pub fn estimate_tokens(text: &str) -> usize`
*   **説明**: 文字コードパターンを読み取り、CJK文字（日本語、中国語、韓国語）は1文字あたり1.5～2トークン、英数字 ASCII はおよそ4文字あたり1トークンとして高速に見積もり計算を実行します。

#### `tokens_to_chars`
*   **シグネチャ**: `pub const fn tokens_to_chars(tokens: usize) -> usize`
*   **説明**: 指定されたトークン制限量を、文字列操作用の最大許容文字数にマッピング逆算します。

#### `truncate_to_tokens`
*   **シグネチャ**: `pub fn truncate_to_tokens(text: &str, max_tokens: usize) -> String`
*   **説明**: 文字列を指定トークン量に収まるようにトリミングスライスします。UTF-8 マルチバイト文字の境界が破損しないように保護しながら処理します。

---

## 2. 優先度に基づくプロンプトパッキング (`budget.rs`)

`pack_prompt` は、システムプロンプトの各ブロックを設定された `ContextBudget` トークン量の境界内に整理・格納します。

#### `from_config`
*   **シグネチャ**: `pub const fn from_config(config: &ContextConfig) -> Self`
*   **説明**: 設定パラメータから基準トークン枠を組み立てます。

#### `from_config_and_hints`
*   **シグネチャ**: `pub const fn from_config_and_hints(config: &ContextConfig, hints: &RecallBudgetHints) -> Self`
*   **説明**: RAG 想起の要求バッファやメタデータ指示値を反映して、現在の発話ターンで利用可能な詳細トークン閾値を構成します。

#### `budget_for`
*   **シグネチャ**: `const fn budget_for(&self, kind: PromptSectionKind) -> usize`
*   **説明**: プロンプトセクションごとの最大許容トークン制限枠を返します。

#### `validate_context_config`
*   **シグネチャ**: `pub fn validate_context_config(config: &ContextConfig) -> Result<(), CognitionError>`
*   **説明**: 設定ファイルのトークン設定値が論理的破綻（セクション合計が最大制限を大幅に超過しているなど）していないか検査します。

#### `sort_memories_for_drop`
*   **シグネチャ**: `fn sort_memories_for_drop(memories: &mut [RecalledMemory])`
*   **説明**: トークン圧迫時に破棄するメモリの順序をソートします。信頼度が低く、ピン留めされていない想起項目が優先して破棄されます。

#### `memory_section_body`
*   **Signature**: `fn memory_section_body(memories: &[RecalledMemory]) -> String`
*   **Description**: 想起された複数のメモリ行を、マークダウンリストの結合テキストにフォーマットします。

#### `set_section_body`
*   **Signature**: `fn set_section_body(sections: &mut [PromptSection], kind: PromptSectionKind, body: String)`
*   **Description**: 特定セクションの文字列をバインド更新します。

#### `estimate_history_tokens`
*   **Signature**: `fn estimate_history_tokens(history: &[HistoryEntry]) -> usize`
*   **Description**: 会話履歴スレッド全体の合計トークン量を算出します。

#### `trim_history_to_budget`
*   **Signature**: `fn trim_history_to_budget(history: &mut Vec<HistoryEntry>, max_tokens: usize) -> usize`
*   **Description**: 履歴が指定制限トークン内に収まるまで、最も古いメッセージレコードから順にトリミングして削減します。

#### `build_sections`
*   **Signature**: `fn build_sections(input: &PackInput, budget: &ContextBudget) -> Vec<PromptSection>`
*   **Description**: プロンプトを構築するための基礎的なマークダウンセクションの配列を組み立てます。

#### `apply_section_budget`
*   **Signature**: `fn apply_section_budget(section: &mut PromptSection)`
*   **Description**: 個別のセクションに割り当てられたトークン閾値チェックを適用し、オーバー分をトリミングします。

#### `section_token_total`
*   **Signature**: `fn section_token_total(sections: &[PromptSection]) -> usize`
*   **Description**: 全セクションの推定トークン量の合計値を集計します。

#### `pack_prompt`
*   **シグネチャ**: `pub fn pack_prompt(input: PackInput, budget: &ContextBudget) -> PackedPrompt`
*   **プロセス**:
    1.  全セクションの合計トークン見積もり値を集計します。
    2.  トークン予算を超過している場合は、以下の逆優先順位に従ってセクションを段階的にトリミングまたは完全に削除（ドロップ）します：
        -   `StyleExamples` (最初期に完全に破棄)
        -   `RecalledMemories` (信頼度の低い順に段階的に削減・ドロップ)
        -   `ActiveCommitments` (約束)
        -   `SceneSummary` / `EmotionSummary`
        -   `History` (古い会話レコードから順にトリミング)
        -   `BehaviorContract` (指示ノート)
    3.  最も重要なアイデンティティプロンプトや直近のユーザー入力テキストが安全に収まるか検証します。
    4.  最終的なプロンプトの構成結果と、トリミングの履歴を示すメタデータを `PackedPrompt` として返します。

#### `classify_recalled_memories_owned`
*   **Signature**: `fn classify_recalled_memories_owned(recalled: &[RecalledMemory]) -> (Vec<RecalledMemory>, Vec<RecalledMemory>, Vec<RecalledMemory>)`
*   **Description**: 想起されたメモリ候補を、約束、セマンティックメモリ、およびエピソードメモリに分類整理します。

---

## 3. スライド窓式セッション圧縮処理 (`compression.rs`)

会話が長くなりコンテキストサイズの上限を超えそうになった場合、古い対話履歴を LLM で要約し、メモリ消費を最適化します。

#### `as_i32` / `from_i32`
*   **Signature**: `pub const fn as_i32(self) -> i32` (および `from_i32` 変換)
*   **Description**: 圧縮の深さレベル（CompressionLevel）をシリアライズするための整数値マッピングです。

#### `compression_has_usable_summary`
*   **Signature**: `pub fn compression_has_usable_summary(result: &CompressionResult) -> bool`
*   **Description**: 要約生成タスクの結果が空でなく、正しく利用可能なものか検証します。

#### `execute_compression`
*   **Signature**: `pub async fn execute_compression(store: Arc<MemoryStore>, provider: Arc<dyn LlmProvider>, input: CompressionTaskInput) -> Result<CompressionResult, CognitionError>`
*   **Description**: 会話履歴を要約し、データベースに保存してメモリを再配置するタスクの完了を待機するコア実行関数です。

#### `spawn_compression_task`
*   **Signature**: `pub fn spawn_compression_task(pending: &mut Option<PendingCompressionTask>, store: Arc<MemoryStore>, provider: Arc<dyn LlmProvider>, input: CompressionTaskInput)`
*   **Description**: 実行スレッドをブロックしないように、会話履歴の圧縮要約タスクをバックグラウンドのスレッドプールに非同期で投入（Spawn）します。

#### `poll_compression_result`
*   **Signature**: `pub fn poll_compression_result(pending: &mut Option<PendingCompressionTask>) -> Option<Result<CompressionResult, CognitionError>>`
*   **Description**: バックグラウンドで実行されている圧縮タスクが完了しているかノンブロッキングでポーリング（Poll）確認します。

#### `evaluate_compression_trigger`
*   **Signature**: `pub fn evaluate_compression_trigger(config: &ContextConfig, turn_count: usize, history_len: usize) -> Option<CompressionReason>`
*   **Description**: 会話ターン数またはコンテキストトークンサイズが設定値の圧縮トリガー限界に達しているかを判定します。

#### `load_active_scene_summary`
*   **Signature**: `pub async fn load_active_scene_summary(store: &MemoryStore, session_id: &str) -> Result<Option<ActiveSceneSummary>, CognitionError>`
*   **Description**: データベースから、該当する対話セッションの最新の圧縮シーン要約テキストをロードします。

#### `run_compression`
*   **Signature**: `async fn run_compression(store: Arc<MemoryStore>, provider: Arc<dyn LlmProvider>, input: CompressionTaskInput) -> Result<CompressionResult, CognitionError>`
*   **Description**: バックグラウンド圧縮タスクの実装処理です。古い対話履歴データをシリアライズして LLM プロバイダで要約を生成し、SQLite の `memory_spans` に保存した上で、会話メモリセッションの履歴リストから該当する過去のレコード行を切り捨てて削除します。

#### `render_turn_excerpt`
*   **Signature**: `fn render_turn_excerpt(turns: &[HistoryEntry]) -> String`
*   **Description**: 要約作成 LLM に渡すために、古い履歴レコードをダイアログ形式のトランスクリプトテキストに変換します。

#### `summarize_span`
*   **Signature**: `async fn summarize_span(provider: &dyn LlmProvider, character_name: &str, user_name: &str, excerpt: &str, level: CompressionLevel, timeout_secs: u64) -> Option<String>`
*   **Description**: LLM プロバイダを呼び出し、対話抜粋テキストを指定の圧縮レベル（概要のみ、または詳細）で要約します。

#### `maybe_roll_up_chapter`
*   **Signature**: `pub async fn maybe_roll_up_chapter(store: &MemoryStore, provider: Arc<dyn LlmProvider>, session_id: &str, character_name: &str, user_name: &str, config: &ContextConfig) -> Result<Option<CompressionResult>, CognitionError>`
*   **Description**: 複数のシーン要約が増えて上限を超えた場合に、それらをさらに統合して大まかな「章（Chapter）サマリー」へとロールアップして長期圧縮を実行します。

---

## 4. ファサードおよびモジュールメソッド (`mod.rs`)

#### `validate_config`
*   **シグネチャ**: `pub fn validate_config(config: &ContextConfig) -> Result<(), CognitionError>`
*   **説明**: コンテキスト構成パラメータの健全性を外部からチェックします。

#### `evaluate_compression_trigger` (ファサード)
*   **シグネチャ**: `pub fn evaluate_compression_trigger(config: &ContextConfig, turn_count: usize, history_len: usize) -> Option<CompressionReason>`
*   **説明**: 履歴要約をトリガーすべきか判定する外部インターフェースです。

#### `load_scene_summary`
*   **シグネチャ**: `pub async fn load_scene_summary(ctx: TurnContext<'_>) -> Result<Option<ActiveSceneSummary>, CognitionError>`
*   **説明**: 現在のアクター実行セッションに対応する有効なシーン要約を読み込みます。
