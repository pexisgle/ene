# `MemoryWriter` / 長期メモリ抽出および忘却仕様

このドキュメントでは、Ene のバックグラウンドメモリ統合システムについて詳細に定義します。正規表現によるルールベース抽出、LLM による連想事実抽出、重複および矛盾の仲裁（Memory Arbiter）、および時間経過に伴う自然忘却減衰（Decay）プロセスが含まれます。

---

## 1. データ構造

### `MemoryWriteProviders<'a>` (パブリック / 構造体)
バックグラウンド実行スレッドに提供される推論サービスプロバイダ：
*   `llm: Option<&'a dyn LlmProvider>`: 構造化 JSON 事実抽出用の LLM インスタンス。
*   `embedder: Option<&'a dyn EmbeddingProvider>`: セマンティック重複判定時の類似度計算用埋め込みプロバイダ。

### `TurnInput` (プライベート / ターン対話情報)
*   `user_message: &str`
*   `assistant_message: &str`
*   `tool_results: &[ToolResultSummary]`: 該当ターン中に実行されたツールの完了結果ログリスト。

---

## 2. メモリ統合ライフサイクル (`MemoryWriter`)

#### `write_memories`
*   **シグネチャ**: `pub async fn write_memories(store: &MemoryStore, config: &MindConfig, input: &PostTurnInput<'_>, providers: MemoryWriteProviders<'_>) -> Result<(), CognitionError>`
*   **プロセス**:
    1.  **ルールベース（決定論的）抽出**: 対話内の明示的なコマンド（「～を覚えておいて」など）やツール実行ログから記憶候補を作成します。
    2.  **LLM による抽出**: LLM に会話のコンテキストを入力し、新しい事実情報の抽出候補をリクエストします。制限時間内に応答が得られない、またはエラーになった場合はルールベースの候補のみを採用します。
    3.  **仲裁（Arbitration）**: 抽出されたすべてのメモリ候補とデータベースに存在する既存メモリを照合し、重複がないか、また矛盾がないか評価します。
    4.  **保存・ベクトル化**: 承認された新しいメモリの埋め込みベクトルを計算し、レコードとともに SQLite にトランザクションで書き込みます。

#### `apply_forgetting`
*   **シグネチャ**: `pub async fn apply_forgetting(store: &MemoryStore, config: &MindConfig, input: &PostTurnInput<'_>) -> Result<(), CognitionError>`
*   **説明**: 自然忘却減衰パイプライン（`ForgettingLifecycle::apply`）を呼び出し、長い間アクセスされていない古い記憶レコードを `Active` から `Faded`（減衰）へ、最終的には `Archived`（アーカイブ）状態に移行します。

#### `finalize_turn`
*   **シグネチャ**: `pub async fn finalize_turn(store: &MemoryStore, _config: &MindConfig, input: &PostTurnInput<'_>) -> Result<(), CognitionError>`
*   **説明**: チャットターンの終了時に同期的に実行され、会話の生対話データを `conversation_logs` に書き込み、クリアされたタスク（Commitment）の状態を更新します。

#### `after_turn`
*   **シグネチャ**: `pub async fn after_turn(store: &MemoryStore, config: &MindConfig, input: PostTurnInput<'_>, providers: MemoryWriteProviders<'_>) -> Result<(), CognitionError>`
*   **説明**: ターンの終了後に、非同期でバックグラウンドメモリ統合タスクをディスパッチします。

#### `build_semantic_matches`
*   **シグネチャ**: `async fn build_semantic_matches(store: &MemoryStore, embedder: Option<&dyn EmbeddingProvider>, config: &crate::config::MindMemoryConfig, character_id: &str, user_id: &str, candidates: &[candidate::MemoryCandidate], similarity_threshold: f32) -> Result<HashMap<usize, Vec<SemanticMatch>>, CognitionError>`
*   **説明**: 新たに抽出されたメモリ候補の埋め込みベクトルをバッチ計算し、類似している既存レコードを SQLite から検索します。

#### `sanitize_ref`
*   **シグネチャ**: `fn sanitize_ref(raw: &str) -> String`
*   **説明**: メモリソースの参照識別子から空白を除去し、正規化します。

#### `locale_from_classifier_language`
*   **シグネチャ**: `const fn locale_from_classifier_language(lang: &str) -> candidate::Locale`
*   **説明**: 言語設定から、抽出処理用のロケール情報（`Locale::Ja` または `Locale::En`）に変換します。

#### `record_arbiter_outcomes`
*   **シグネチャ**: `fn record_arbiter_outcomes(input: &PostTurnInput<'_>, applied: &[crate::memory_writer::AppliedDecision], summary: &mut ArbiterOutcomeSummary)`
*   **説明**: メモリ仲裁の結果統計データを指標モニタリング用に集計・記録します。

---

## 3. ルールベース（決定論的）抽出 (`deterministic.rs` & `tool_grounding.rs`)

#### `extract`
*   **シグネチャ**: `pub fn extract(turn: &TurnInput<'_>, locale: Locale, min_confidence: f32) -> Result<Vec<MemoryCandidate>, CognitionError>`
*   **説明**: ユーザー発話テキストから、明示的な記憶要求（「～を覚えておいて」など）の正規表現パターンに基づいて、決定論的にメモリ候補を抽出します。

#### `extract_with_tool_grounding`
*   **シグネチャ**: `pub fn extract_with_tool_grounding(turn: &TurnInput<'_>, locale: Locale, min_confidence: f32, tool_grounding_cfg: &ToolGroundingConfig) -> Result<Vec<MemoryCandidate>, CognitionError>`
*   **説明**: 明示的な記憶要求と、ツールの実行ログに基づくシステム候補抽出を統合します。

#### `ja_explicit_remember` / `en_explicit_remember`
*   **シグネチャ**: `fn ja_explicit_remember(user_msg: &str, _asst_msg: &str, _tool_results: &[ToolResultSummary]) -> Option<MemoryCandidate>` (および EN 版)
*   **説明**: ユーザーがアクターに対して事実の暗記を指示したパターンを検出する言語固有の抽出エンジンです。

#### `ja_forget_request` / `en_forget_request`
*   **シグネチャ**: `fn ja_forget_request(user_msg: &str, _asst_msg: &str, _tool_results: &[ToolResultSummary]) -> Option<MemoryCandidate>` (および EN 版)
*   **説明**: ユーザーが特定の記憶の忘却・削除を指示したパターンを検出します。

#### `summarize_tool_result`
*   **シグネチャ**: `pub fn summarize_tool_result(tool_name: &str, raw_output: &str, success: bool, max_summary_chars: usize) -> ToolResultSummary`
*   **説明**: ツールの実行戻り値データをメモリ記述用に簡潔にサマライズします。

#### `extract_tool_candidates`
*   **シグネチャ**: `pub fn extract_tool_candidates(tool_results: &[ToolResultSummary], cfg: &ToolGroundingConfig) -> Vec<MemoryCandidate>`
*   **説明**: 実行されたツールの完了結果（ファイルパスの書き込み操作など）に基づいて、実用的な記憶候補を自動生成します。

#### `normalize_tool_output`
*   **Signature**: `fn normalize_tool_output(raw_output: &str) -> String`
*   **Description**: ツールの出力を文字トリミングして整形します。

#### `is_screenshot_payload`
*   **Signature**: `fn is_screenshot_payload(result: &str) -> bool`
*   **Description**: ツールの戻り値に画像バイナリやスクリーンショット情報が含まれているかを判定します。

---

## 4. LLM による事実抽出 (`llm.rs`)

#### `extract`
*   **シグネチャ**: `pub async fn extract(provider: &dyn LlmProvider, turn: &TurnInput<'_>, locale: Locale) -> Result<Vec<MemoryCandidate>, CognitionError>`
*   **説明**: LLM プロバイダに対して構造化された指示プロンプトを送信し、対話履歴全体から暗黙的に示されたユーザーの設定や事実を抽出します。

#### `extract_with_timeout`
*   **シグネチャ**: `pub async fn extract_with_timeout(provider: &dyn LlmProvider, turn: &TurnInput<'_>, locale: Locale, timeout_secs: u64, pattern_hints: &[MemoryCandidate]) -> Result<Vec<MemoryCandidate>, CognitionError>`
*   **説明**: LLM の抽出タスクにタイムアウト制御を課し、ネットワーク遅延などによるバックグラウンドタスクの無制限の停滞を回避します。

#### `format_pattern_hints`
*   **Signature**: `fn format_pattern_hints(hints: &[MemoryCandidate]) -> String`
*   **Description**: 抽出の質を向上させるために、LLM プロンプト内に例示として提示するヒント情報をフォーマットします。

#### `build_conversation_text`
*   **Signature**: `fn build_conversation_text(turn: &TurnInput<'_>) -> String`
*   **Description**: 直近の対話ターンデータを、抽出プロンプトに挿入するトランスクリプトテキストとしてフォーマットします。

#### `extraction_schema`
*   **Signature**: `fn extraction_schema() -> serde_json::Value`
*   **Description**: LLM に JSON 出力を強制させるための構造化データスキーマを定義します。

#### `parse_candidates_json`
*   **Signature**: `fn parse_candidates_json(raw: &str, locale: Locale) -> Result<Vec<MemoryCandidate>, CognitionError>`
*   **Description**: LLM から返された応答テキストを解析し、メモリ候補配列にデシリアライズします。

#### `raw_to_candidate`
*   **Signature**: `fn raw_to_candidate(raw: RawCandidate, locale: Locale) -> MemoryCandidate`
*   **Description**: デシリアライズされた候補オブジェクトのパラメータ整合性をチェック・クレンジングします。

#### `locale_mismatch`
*   **Signature**: `fn locale_mismatch(text: &str, locale: Locale) -> bool`
*   **Description**: 抽出結果の言語属性がロケール指定と矛盾していないか評価します。

---

## 5. 重複と矛盾の仲裁 (`arbiter.rs`)

新しく抽出されたメモリ候補を既存データベースのメモリと照合し、書き込みアクションを決定します。

#### `evaluate_all`
*   **シグネチャ**: `pub fn evaluate_all(candidates: &[MemoryCandidate], existing: &[MemoryItem], ctx: &ArbiterContext<'_>) -> Vec<CandidateDecision>`
*   **説明**: すべての抽出された候補レコードを精査し、挿入（Insert）、無視（Ignore）、上書き（Supersede）、または矛盾（Contradiction）のフラグを設定します。

#### `evaluate_one`
*   **シグネチャ**: `pub(crate) fn evaluate_one(candidate: &MemoryCandidate, existing: &[MemoryItem], ctx: &ArbiterContext<'_>, semantic_matches: &[SemanticMatch]) -> CandidateDecision`
*   **説明**: 個々の候補について、セマンティック重複度の評価、明示的な削除要求の有無、および信頼度のスコア境界をチェックします。

#### `evaluate_deletion`
*   **シグネチャ**: `fn evaluate_deletion(candidate: &MemoryCandidate, existing: &[MemoryItem], ctx: &ArbiterContext<'_>) -> CandidateDecision`
*   **説明**: メモリ削除要求コマンドの対象となる既存メモリレコードを特定し、アーカイブ（Archived）状態へ移行する決定フラグを生成します。

#### `validate_candidate`
*   **Signature**: `fn validate_candidate(candidate: &MemoryCandidate, ctx: &ArbiterContext<'_>) -> Option<ArbiterReason>`
*   **Description**: メモリの重要度スコアが極端に低くないか、また引用元対話（Quote）の範囲が妥当かを検証します。

#### `check_semantic_matches`
*   **Signature**: `fn check_semantic_matches(candidate: &MemoryCandidate, semantic_matches: &[SemanticMatch], ctx: &ArbiterContext<'_>, existing: &[MemoryItem]) -> Option<CandidateDecision>`
*   **Description**: ベクトル検索によって見つかった既存類似メモリと一致度を比較し、同一の事実であれば書き込みを無視（Ignore）または更新判定します。

#### `check_contradiction`
*   **Signature**: `fn check_contradiction(candidate: &MemoryCandidate, existing: &[MemoryItem], ctx: &ArbiterContext<'_>) -> Option<CandidateDecision>`
*   **Description**: 新しい事実が既存の記憶レコードと矛盾した内容を主張していないかチェックします。

#### `contradiction_decision`
*   **Signature**: `fn contradiction_decision(candidate: &MemoryCandidate, existing: &MemoryItem, ctx: &ArbiterContext<'_>, supersede_code: ArbiterReasonCode, dispute_code: ArbiterReasonCode, ask_code: ArbiterReasonCode, detail: String) -> Option<CandidateDecision>`
*   **Description**: 矛盾が検出された場合の判定ルールを解決します。新しい事実の方が圧倒的に信頼できる場合は「上書き（Supersede）」、両者の確証が得られない場合は状態を「論争中（Disputed）」に変更し、対話プロンプト上でユーザーに事実確認を行うための問いかけをトリガーします。

#### `apply_decisions`
*   **シグネチャ**: `pub async fn apply_decisions(store: &MemoryStore, decisions: &[CandidateDecision]) -> Result<Vec<AppliedDecision>, CognitionError>`
*   **説明**: 決定されたアクションリストを一括トランザクションで SQLite にコミットします。

#### `apply_one`
*   **Signature**: `async fn apply_one(store: &MemoryStore, decision: &CandidateDecision) -> Result<AppliedDecision, CognitionError>`
*   **Description**: 個別のアクション（挿入、ステータス変更、論争フラグの設定など）を実行します。

#### `candidate_to_new_item`
*   **Signature**: `fn candidate_to_new_item(candidate: &MemoryCandidate, ctx: &ArbiterContext<'_>) -> NewMemoryItem`
*   **Description**: メモリ候補オブジェクトをデータベースに保存するための形式に変換します。

#### `passes_validation`
*   **Signature**: `fn passes_validation(candidate: &MemoryCandidate, ctx: &ArbiterContext<'_>) -> bool`
*   **Description**: 基本的なデータ検証に合格しているか判定します。

#### `normalize_text`
*   **Signature**: `fn normalize_text(s: &str) -> String`
*   **Description**: 比較処理用にテキストを正規化クレンジングします。

#### `dedup_key`
*   **Signature**: `fn dedup_key(candidate: &MemoryCandidate) -> (MemoryKind, String)`
*   **Description**: 同一コンテキスト内の一時的な重複排除用ハッシュキーを生成します。

#### `source_quote_valid`
*   **Signature**: `fn source_quote_valid(candidate: &MemoryCandidate, turn: &TurnInput<'_>) -> bool`
*   **Description**: 引用された元の会話スニペットが、実際の対話テキスト中に存在しているか検証します。

#### `find_exact_duplicate`
*   **Signature**: `fn find_exact_duplicate(candidate: &MemoryCandidate, existing: &[MemoryItem]) -> Option<i64>`
*   **Description**: 既存の記憶と完全に一致するレコードが存在するかどうか検索します。

#### `find_deletion_targets`
*   **Signature**: `fn find_deletion_targets(target: &str, existing: &[MemoryItem]) -> Vec<i64>`
*   **Description**: 忘却要求の記述に該当する既存メモリ ID を検索・収集します。
