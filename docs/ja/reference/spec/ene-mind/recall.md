# `RecallPlanner` & ハイブリッド長期記憶検索仕様

本ドキュメントでは、ユーザーからの入力メッセージおよび現在の感情状態を分析して長期記憶（エピソード記憶、セマンティックファクト、ルール、対話スタイルの抽出）を回収（Recall）する計画と実行の仕様について詳細に定義します。

---

## 1. データ構造と列挙型

### `RecallPlan` (公開 / 構造体)
アクターにデータベース検索を指示するための非バイナリな検索計画。
*   `current_topic: String`: 解析された現在の対話トピック。
*   `semantic_queries: Vec<String>`: セマンティックメモリ検索用の展開クエリ群。
*   `episodic_queries: Vec<String>`: エピソードメモリ検索用のクエリ群。
*   `required_kinds: Vec<MemoryKind>`: 検索要求するメモリの種別（`Semantic`, `Episodic`等）。
*   `scope: RecallScopeFilter`: クライアントキャラクターIDおよびユーザー名によるフィルタ。
*   `budget: RecallBudgetHints`: 許容トークン上限および最大検索取得件数。
*   `search: RecallSearchHints`: 最も優先される一次検索文字列、類似度しきい値、時間減衰半減期、および検索用感情PAD値。

### `RecalledMemory` (公開 / 構造体)
検索結果として取得された、理由付け付きの記憶データ。
*   `item: MemoryItem`: 取得されたメモリレコード実体。
*   `reason: RecallReason`: その記憶がなぜ回収されたかの説明理由。

### `RecallReason` (公開 / 列挙型)
記憶回収の選定理由。
*   `SimilarTopic`: ベクトル類似度によるトピックの一致。
*   `KeywordMatch`: キーワード（キーワード一致）によるヒット。
*   `EmotionalMatch`: 現在の感情（valence 等）に近いエピソード。
*   `CommitmentLink`: 未完了タスク・約束に紐づく事実。
*   `RecencyFallback`: 話題がなかった際のフォールバックとしての直近記憶。
*   `Constant`: キャラクター設定等の常時展開ルール。

---

## 2. 計画フェーズ (`RecallPlanner`)

`RecallPlanner` は、データベースや通信を直接呼び出さない、決定論的な検索クエリ計画生成コンポーネントです。

### 主要メソッド

#### `plan`
*   **シグネチャ**: `pub fn plan(input: &RecallPlannerInput<'_>, options: &RecallPlannerOptions) -> Result<RecallPlan, CognitionError>`
*   **処理手順**:
    1.  `current_topic` にて、ユーザー入力テキスト、直近履歴、シーンサマリーを元に対話トピックを抽出。
    2.  `infer_intents` により、トピック文字列のキーワード（「天気」「あなた」「約束」等）と現在のPAD感情値から、意図（`RecallIntent`）を判定。
    3.  `kinds_for_intents` により、意図から `MemoryKind`（感情が動いた話題なら `Episodic`、タスクなら `Semantic` など）を逆引き。
    4.  意図と未完了タスク状況を元に、検索文字列候補（`semantic_queries`、`episodic_queries`）を生成。
    5.  検索設定情報を含む `RecallPlan` を返却。

#### `to_memory_search_options`
*   **シグネチャ**:
    ```rust
    pub fn to_memory_search_options<'a>(
        plan: &'a RecallPlan,
        query_embedding: &'a [f32],
        model_name: &'a str,
        now: DateTime<Utc>,
        memory: &MindMemoryConfig,
    ) -> Query<'a>
    ```
*   **解説**: 生成された `RecallPlan` を、`ene-store` が解釈可能な SQLite ハイブリッド類似度検索クエリ `Query` に変換します。この際、類似度計算用のクエリベクトルや時間減衰計算用の現在時刻（`now`）をパラメータとして差し込みます。

---

## 3. 実行フェーズ (`execute_hybrid_recall`)

`runner.rs` に定義され、データベース検索、多様化フィルタ（MMR）、およびキャラクター固有設定（Lorebook）の結合を担当します。

### `execute_hybrid_recall`
*   **シグネチャ**: `pub async fn execute_hybrid_recall(config: &MindConfig, input: &ExecuteRecallInput<'_>) -> Result<(RecallPlan, Vec<RecalledMemory>), CognitionError>`
*   **制御フロー**:
    1.  `CommitmentLedger::list_active` によりアクティブな未完了タスクを取得。
    2.  `RecallPlanner::plan` を呼び出し `RecallPlan` を構築。
    3.  `RecallPlanner::to_memory_search_options` により `Query` を生成。
    4.  データベース (`MemoryStore::search`) を実行し、スコア付きメモリ候補（`ScoredMemory`）の初期プールを取得。
    5.  **多様化処理 (MMR Diversification)**:
        `MemoryDiversifyPipeline::diversify` を実行。最大境界関連性（MMR）アルゴリズムに基づき、回収結果の中で内容が極めて酷似している重複メモリを排除し、多様で有用な記憶のみに絞り込みます。
    6.  **結果マッピング**:
        `RecallResultMapper::map` で、類似度や属性から `RecallReason` を決定論的に判定して `RecalledMemory` に変換。
    7.  **キャラクター設定マージ**:
        `merge_lorebook_recall` により、キャラクターカードV3で設定されている「特定の単語に反応して読み込む記憶（Lorebook）」や「常時ロードされる世界観ルール（Constant）」を、回収メモリリストに差し込んで統合。
    8.  **アクセス数の記録**:
        回収されたメモリレコードのアクセス回数・最終アクセス時刻をデータベースに更新 (`bump_typed_memory_access`) し、忘却モデルにおける減衰を防ぐためにアクセススコアを底上げ。

---

## 4. プロンプト生成フォーマッタ (`prompt_qualifier.rs`)

回収された `RecalledMemory` リストは、そのままでは LLM に入力できません。LLM が事実関係と自身の記憶として適切に認識できるよう、システムプロンプトの1セクションへと変換します。

*   `format_recalled_content(memories: &[RecalledMemory]) -> String`:
    -   記憶項目を Markdown 形式の箇条書きに変換。
    -   メタデータとして、記憶のピン留め状態、感情アノテーション、および回収された理由（`RecallReason`）をタグ形式で前置します（例: `[EPISODIC (recalled because: similar_topic)] content`）。
