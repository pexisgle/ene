# `RecallPlanner` およびハイブリッドメモリ想起仕様

このドキュメントは、Ene のロングタームメモリ想起システムについて定義します。ユーザー入力、アクティブな会話トピック、意図（Intent）、および感情状態を分析し、ハイブリッドデータベースクエリを策定して、ベクトル類似度検索を実行し、取得されたメモリを MMR アルゴリズムで多様化処理するフローを制御します。

---

## 1. データ構造

### `RecallPlan` (パブリック / 構造体)
想起タスクを定義する計画パラメータ：
*   `current_topic: String`: 解析されたアクティブな会話トピック。
*   `semantic_queries: Vec<String>`: セマンティックメモリの検索用のクエリリスト。
*   `episodic_queries: Vec<String>`: エピソード（会話セグメント）メモリの検索用のクエリリスト。
*   `required_kinds: Vec<MemoryKind>`: 検索対象となる記憶の分類（`Semantic` または `Episodic` など）。
*   `scope: RecallScopeFilter`: アクターおよびユーザー ID のスコープフィルタ。
*   `budget: RecallBudgetHints`: トークンサイズと検索結果の取得最大件数。
*   `search: RecallSearchHints`: 主なクエリ文、適合閾値、時間減衰の比率、および感情属性の重み。

---

## 2. 想起計画フェーズ (`RecallPlanner`)

`RecallPlanner` は、ターンコンテキストに基づいて想起計画を策定する決定論的モジュールです。

#### `from_config`
*   **シグネチャ**: `pub fn from_config(context: &ContextConfig, memory: &MindMemoryConfig) -> Self`
*   **説明**: トークンサイズや自然忘却の減衰パラメータなどを指定して `RecallPlanner` を構築します。

#### `plan`
*   **シグネチャ**: `pub fn plan(input: &RecallPlannerInput<'_>, options: &RecallPlannerOptions) -> Result<RecallPlan, CognitionError>`
*   **プロセス**:
    1.  ユーザーの直近の発言と直前の会話履歴スレッドからアクティブな会話トピックを解決します。
    2.  `infer_intents` を呼び出し、キーワードマッチングおよび感情状態から意図（`RecallIntent`）を判定します。
    3.  `kinds_for_intents` を使用して、クエリ対象とするメモリ分類（エピソードメモリ、セマンティックメモリなど）を決定します。
    4.  アクティブな約束（Commitments）と意図から検索クエリ文字列を構築します。
    5.  結果を `RecallPlan` 計画書としてまとめて返します。

#### `to_query`
*   **シグネチャ**: `pub fn to_query<'a>(plan: &'a RecallPlan, embedding: Option<&'a [f32]>, model_name: &'a str, now: DateTime<Utc>, memory: &MindMemoryConfig) -> Query<'a>`
*   **説明**: 想起計画情報をデータベースが実行可能な汎用クエリ構造体へと展開します。

#### `to_memory_search_options`
*   **シグネチャ**: `pub fn to_memory_search_options<'a>(plan: &'a RecallPlan, query_embedding: &'a [f32], model_name: &'a str, now: DateTime<Utc>, memory: &MindMemoryConfig) -> Query<'a>`
*   **説明**: 想起計画情報を SQLite (sqlite-vec) 接続プールで実行可能な `Query` オプション形式に変換します。クエリ文字列の埋め込みベクトルデータ、減衰計算用の現在時刻（`now`）、および関連するオプションメタデータをインジェクトします。

#### `explain_results`
*   **シグネチャ**: `pub fn explain_results(scored: Vec<ScoredMemory>) -> Vec<RecalledMemory>`
*   **説明**: クエリ結果の類似度スコアに基づいて、メモリの想起判定理由（想起理由コード）を付与します。

#### `semantic_queries`
*   **シグネチャ**: `fn semantic_queries(topic: &str, intents: &[RecallIntent], commitments: &[ActiveCommitmentPrompt]) -> Vec<String>`
*   **説明**: トピック、ユーザーの意図、および約束リストを組み合わせて、セマンティックメモリテーブル用の検索クエリを生成します。

#### `episodic_queries`
*   **シグネチャ**: `fn episodic_queries(topic: &str, recent_turns: &[super::input::RecallTurn<'_>], intents: &[RecallIntent]) -> Vec<String>`
*   **説明**: 直近の会話データや意図から、エピソードメモリテーブル用の検索用クエリを作成します。

#### `query_affect`
*   **シグネチャ**: `fn query_affect(state: &AffectState) -> Option<AffectAnnotation>`
*   **説明**: 現在の感情の PAD 座標データを、想起検索に適用する感情重みのアノテーションにマッピングします。

#### `clamp_unit_signed`
*   **シグネチャ**: `const fn clamp_unit_signed(value: f32) -> f32`
*   **説明**: 入力 float 値を `-1.0` から `1.0` の範囲にクランプするユーティリティです。

#### `push_query`
*   **シグネチャ**: `fn push_query(queries: &mut Vec<String>, query: &str)`
*   **説明**: 空文字や重複を排除して、検索用クエリ配列に安全に文字列を追加します。

---

## 3. トピックと意図の分類 (`topic.rs` および `intent.rs`)

#### `current_topic`
*   **シグネチャ**: `pub fn current_topic(user_input: &str, recent_turns: &[RecallTurn<'_>], scene_summary: Option<&str>) -> Option<String>`
*   **説明**: 会話履歴スレッド、現在シーンの要約、およびユーザー入力からトピック名を推定します。

#### `normalize_text`
*   **シグネチャ**: `pub fn normalize_text(text: &str) -> Option<String>`
*   **説明**: 大文字小文字の変換、前後の空白の除去、および記号のクリーンアップを行います。

#### `recent_user_turn`
*   **シグネチャ**: `pub fn recent_user_turn(recent_turns: &[RecallTurn<'_>]) -> Option<String>`
*   **説明**: 履歴メッセージ配列から最後にユーザーが発言したテキストを抽出します。

#### `contains_case_insensitive`
*   **シグネチャ**: `fn contains_case_insensitive(haystack: &str, needle: &str) -> bool`
*   **説明**: 大文字小文字を区別せず部分一致判定を行います。

#### `truncate_chars`
*   **シグネチャ**: `fn truncate_chars(text: &str, max_chars: usize) -> String`
*   **説明**: UTF-8 文字境界を考慮して、文字列を指定された最大文字数に安全に切り詰めます。

#### `infer_intents`
*   **シグネチャ**: `pub fn infer_intents(topic: &str, affect: Option<&AffectState>) -> Vec<RecallIntent>`
*   **説明**: トピックのキーワードパターンおよび現在の感情状態を評価し、ユーザーが求めている情報のカテゴリ（ユーザーの背景、アクターの設定、感情的な記憶など）を分類します。

#### `kinds_for_intents`
*   **シグネチャ**: `pub fn kinds_for_intents(intents: &[RecallIntent], has_commitments: bool) -> Vec<MemoryKind>`
*   **説明**: 分類された意図情報から、検索対象とすべきデータベースメモリ種別の優先リストを作成します。

#### `contains_any`
*   **Signature**: `pub fn contains_any(text: &str, needles: &[&str]) -> bool`
*   **Description**: 正規化テキスト中に、いずれかのキーワードが含まれているか判定します。

#### `dedupe_intents`
*   **Signature**: `fn dedupe_intents(intents: &mut Vec<RecallIntent>)`
*   **Description**: 意図リスト内の重複項目をインプレースで削除します。

#### `push_unique`
*   **Signature**: `fn push_unique(kinds: &mut Vec<MemoryKind>, kind: MemoryKind)`
*   **Description**: メモリ種類リストに一意な項目を追加します。

---

## 4. 想起実行フェーズ (`execute_hybrid_recall`)

想起の実行を管理し、DB 検索、MMR 多様化、およびキャラクターカード内のローブックマージを処理します。

#### `execute_hybrid_recall`
*   **シグネチャ**: `pub async fn execute_hybrid_recall(config: &MindConfig, input: &ExecuteRecallInput<'_>) -> Result<(RecallPlan, Vec<RecalledMemory>), CognitionError>`
*   **制御フロー**:
    1.  `CommitmentLedger::list_active` 経由でデータベースから現在のアクティブな約束をロードします。
    2.  `RecallPlanner::plan` を呼び出して `RecallPlan` を作成します。
    3.  `RecallPlanner::to_memory_search_options` を実行して、クエリ構造体を生成します。
    4.  SQLite の接続プールに対してベクトル検索および語彙検索（`MemoryStore::search`）をトリガーし、複数の `ScoredMemory` 候補レコードを収集します。
    5.  **多様化フィルタリング (MMR)**:
        `MemoryDiversifyPipeline::diversify` を呼び出して Maximal Marginal Relevance 多様化を実行し、内容が重複する冗長なドキュメントを切り捨て、トークン予算内でより広範な記憶が LLM に提示されるようにします。
    6.  **想起理由の割り当て**:
        `RecallResultMapper::map` を使用して、各結果に想起判定理由（想起理由コード）を付与します。
    7.  **ローブックのマージ**:
        `maybe_merge_lorebook_recall` を呼び出し、キャラクターカードに定義されている静的な設定情報やキーワードトリガーのローブック（Lorebook）レコードを検索結果にマージします。
    8.  **アクセス履歴の更新 (Decay 防止)**:
        想起されたメモリのアクセス件数カウンタを非同期バンプ処理（`bump_typed_memory_access`）し、自然忘却（Forgetting）による減衰を防ぎます。

#### `maybe_merge_lorebook_recall`
*   **シグネチャ**: `async fn maybe_merge_lorebook_recall(config: &MindConfig, input: &ExecuteRecallInput<'_>, recalled: Vec<RecalledMemory>) -> Result<Vec<RecalledMemory>, CognitionError>`
*   **説明**: 想起されたメモリリストに対し、キャラクターカードの設定キーワードに合致するローブックエントリーをロードしてマージします。

#### `bump_recalled_memory_access`
*   **シグネチャ**: `async fn bump_recalled_memory_access(store: &MemoryStore, recalled: &[RecalledMemory])`
*   **説明**: 想起されたメモリのアクセス履歴タイムスタンプおよび累積カウンタを更新します。

#### `merge_lorebook_recall`
*   **シグネチャ**: `pub async fn merge_lorebook_recall(store: &MemoryStore, character_id: &str, card: Option<&CharacterCardV3>, user_input: &str, recent_turns: &[RecallTurn<'_>], recalled: Vec<RecalledMemory>) -> Result<Vec<RecalledMemory>, CognitionError>`
*   **説明**: トピックおよび履歴テキスト中にキャラクターカードのローブックトリガーワードが含まれているかをスキャンし、一致した項目を抽出してマージします。

#### `recalled_memory_from_item`
*   **シグネチャ**: `fn recalled_memory_from_item(item: MemoryItem) -> RecalledMemory`
*   **説明**: メモリレコードを想起オブジェクトにマッピングし、想起理由を「Constant（不変設定）」に設定します。

#### `lorebook_entry_matches`
*   **シグネチャ**: `fn lorebook_entry_matches(item: &MemoryItem, book: &ene_config::Lorebook, scan_text: &str, regex_cache: &std::collections::HashMap<String, regex::Regex>) -> bool`
*   **説明**: 対象のメモリがローブックの正規表現条件に合致するかチェックします。

---

## 5. 多様化フィルタリング (`diversify.rs`)

想起されたコンテキストが類似した内容で埋め尽くされないように、Maximal Marginal Relevance (MMR) を適用して調整します。

#### `from_config`
*   **シグネチャ**: `pub const fn from_config(config: &MindMemoryConfig) -> Self`
*   **説明**: 多様化クォータスロット設定や類似度閾値などを読み込んで多様化パイプラインを初期化します。

#### `diversify`
*   **シグネチャ**: `pub fn diversify(candidates: Vec<ScoredMemory>, plan: &RecallPlan, options: MemoryDiversifyOptions) -> Vec<ScoredMemory>`
*   **プロセス**:
    1.  指定されたテキスト重複閾値に基づいて、極端に内容が重なるメモリ候補をクラスタリング・重複排除します。
    2.  特定の記憶カテゴリ（約束など）に最低限確保すべき最小クォータスロット量を割り当てます。
    3.  `greedy_mmr` アルゴリズムを適用し、すでに選択された他のコンテキスト情報との類似性を考慮した上で、最も高い多様性効果をもたらす最適なサブセットを選出します。
    4.  選択されたメモリ候補のリストを返します。

#### `truncate`
*   **Signature**: `fn truncate(mut candidates: Vec<ScoredMemory>, limit: usize) -> Vec<ScoredMemory>`
*   **Description**: リストを指定された制限サイズにスライスします。

#### `item_similarity`
*   **Signature**: `fn item_similarity(a: &ScoredMemory, b: &ScoredMemory) -> f32`
*   **Description**: メモリ同士の埋め込みベクトル間のコサイン類似度を測定します。

#### `cluster_dedup`
*   **Signature**: `fn cluster_dedup(mut candidates: Vec<ScoredMemory>, threshold: f32) -> Vec<ScoredMemory>`
*   **Description**: 類似度が近すぎる項目をグループ化し、最もスコアの高い代表値のみを保持して重複排除します。

#### `greedy_mmr`
*   **Signature**: `fn greedy_mmr(pool: &[ScoredMemory], limit: usize, options: MemoryDiversifyOptions) -> Vec<ScoredMemory>`
*   **Description**: Greedy 式の MMR ループを実行し、情報カバレッジを最大化するサブセットを選出します。

#### `mmr_score`
*   **Signature**: `fn mmr_score(candidate: &ScoredMemory, selected: &[ScoredMemory], options: MemoryDiversifyOptions, max_relevance: f32) -> f32`
*   **Description**: クエリ適合度と既存選出メモリ群との相違度を重み付けして候補の MMR スコアを評価します。

#### `source_diversity_bonus`
*   **Signature**: `fn source_diversity_bonus(candidate: &ScoredMemory, selected: &[ScoredMemory], bonus: f32) -> f32`
*   **Description**: まだ十分にカバーされていないカテゴリの記憶ソースに対して追加のスコアボーナスを付与します。

#### `effective_min_slots`
*   **Signature**: `fn effective_min_slots(plan: &RecallPlan, options: MemoryDiversifyOptions, limit: usize) -> Vec<(MemoryKind, usize)>`
*   **Description**: 各カテゴリごとにクォータとして確保すべき最低スロット量を決定します。

#### `apply_kind_quotas`
*   **Signature**: `fn apply_kind_quotas(selected: &mut Vec<ScoredMemory>, pool: &[ScoredMemory], plan: &RecallPlan, options: MemoryDiversifyOptions, limit: usize)`
*   **Description**: 最低限必要なメモリ種別のクォータ枠が確保されるように、不足しているカテゴリの候補を挿入・調整します。

#### `best_pool_candidate_for_kind`
*   **Signature**: `fn best_pool_candidate_for_kind(pool: &[ScoredMemory], selected: &[ScoredMemory], kind: MemoryKind) -> Option<ScoredMemory>`
*   **Description**: 指定されたカテゴリの中で最もスコアの高い未選出の候補を取得します。

#### `lowest_scoring_swappable_index`
*   **Signature**: `fn lowest_scoring_swappable_index(selected: &[ScoredMemory], mins: &[(MemoryKind, usize)]) -> Option<usize>`
*   **Description**: クォータ補填の目的でスワップ（交換）可能な、最もスコアの低い項目のインデックスを特定します。

#### `kind_counts`
*   **Signature**: `fn kind_counts(selected: &[ScoredMemory]) -> std::collections::HashMap<&'static str, usize>`
*   **Description**: リスト内の項目カテゴリ別の個数をカウントしてマッピングを生成します。

---

## 6. プロンプト用フォーマット構築 (`prompt_qualifier.rs`)

#### `format_recalled_content`
*   **シグネチャ**: `pub fn format_recalled_content(memory: &RecalledMemory) -> String`
*   **説明**: メモリ項目を設定メタデータ（記憶ソース、想起理由コードなど）とともにマークダウンのリスト要素形式に文字列化してフォーマットします。

#### `recall_content_qualifier`
*   **シグネチャ**: `pub fn recall_content_qualifier(memory: &RecalledMemory) -> Option<&'static str>`
*   **説明**: メモリ状態が「Faded（減衰）」または適合信頼度が低い場合に、LLM に情報が不確実であることを示すための接頭辞（例: "Uncertain:"）を決定します。
