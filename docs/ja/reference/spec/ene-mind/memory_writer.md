# `MemoryWriter` / 長期記憶抽出 & 減衰忘却仕様

本ドキュメントでは、会話の終了後にバックグラウンドタスクとして実行される長期記憶の自動抽出（記憶の固定化）、重複や矛盾の調停（Memory Arbiter）、および時間の経過に伴う自然忘却（減衰）の処理仕様を詳細に定義します。

---

## 1. データ構造

### `MemoryWriteProviders<'a>` (公開 / 構造体)
記憶の固定化プロセスに必要なAIプロバイダーの参照。
*   `llm: Option<&'a dyn LlmProvider>`: メモリ抽出用の LLM プロバイダー。
*   `embedder: Option<&'a dyn EmbeddingProvider>`: 重複チェックのベクトル類似度判定用プロバイダー。

### `TurnInput` (非公開 / 会話ターン入力データ)
*   `user_message: &str`
*   `assistant_message: &str`
*   `tool_results: &[ToolResultSummary]`: ターン中に実行されたツールの情報。

---

## 2. 記憶固定化ライフサイクル (`MemoryWriter`)

### `write_memories`
*   **シグネチャ**:
    ```rust
    pub async fn write_memories(
        store: &MemoryStore,
        config: &MindConfig,
        input: &PostTurnInput<'_>,
        providers: MemoryWriteProviders<'_>,
    ) -> Result<(), CognitionError>
    ```
*   **制御フロー**:
    1.  **確定条件に基づく抽出 (Deterministic Extraction)**:
        -   `deterministic::extract_with_tool_grounding` を実行。会話テキスト内から正規表現パターン（例: 「〜であることを覚えておいて」等）に合致する記憶指示を抽出。
        -   ツール実行結果から、設定に応じて自動保存対象となる候補（API応答やファイル更新ログなど）を抽出。
    2.  **LLMによる自動抽出 (LLM Extraction)**:
        -   `providers.llm` が利用可能な場合、`llm::extract_with_timeout` を実行。会話全体をLLMに解析させ、キャラクターにとって永続保存すべき重要なファクト（ユーザーの好み、共有エピソード等）を抽出。
        -   抽出結果が空の場合やLLM接続失敗時は、手順1の決定論的抽出結果をバックアップとして適用。
    3.  **重複・矛盾調停 (Memory Arbiter)**:
        -   抽出されたメモリ候補ごとに、既存の記憶と重複していないかベクトル近傍探索で重複検出。
        -   競合する古い記憶が見つかった場合は、確信度（Confidence）の差分を元に上書き（Supersede）、紛争状態（Disputed）、または無視を決定。
    4.  **永続化**:
        -   調停を通った新規/更新メモリのベクトル埋め込みを生成し、SQLiteの `typed_memories` / `memory_embeddings` に保存。

### `apply_forgetting`
*   **シグネチャ**: `pub async fn apply_forgetting(store: &MemoryStore, config: &MindConfig, input: &PostTurnInput<'_>) -> Result<(), CognitionError>`
*   **解説**: 長期記憶の整理タスクを起動。`ForgettingLifecycle::apply` を用いて、時間が経過したアクティブ記憶を「Faded（かすれた記憶）」へ、さらに時間が経過したものを「Archived（保管庫）」へとステータス遷移させます。

### `finalize_turn` (同期的前処理)
*   **シグネチャ**: `pub async fn finalize_turn(store: &MemoryStore, config: &MindConfig, input: &PostTurnInput<'_>) -> Result<(), CognitionError>`
*   **解説**: 今回の会話対話ログ（`conversation_logs`）の保存、およびコミットメントの完了処理などのデータベース永続化を実行します。

---

## 3. 各機能コンポーネント

### 1. 決定論的抽出 (`deterministic.rs` & `tool_grounding.rs`)
*   **テキストパターン**: `MindConfig` で指定された正規表現パターン（言語別）を用いて、ユーザー発話内の明示的な記憶指示をパース。
*   **ツール結果グラウンディング**: ツール実行結果の JSON 内の特定キーやエラー内容から、記憶候補（例: 「fs.writeでファイルを保存した事実」等）を自動生成。

### 2. LLM抽出 (`llm.rs`)
*   **制約プロンプト**: LLMに対して、JSON スキーマに厳格に従ったメモリ候補リスト（Title、Content、Confidence、EmotionalImpact 等）を返却するよう要求。
*   **タイムアウト制御**: 設定された `extraction_timeout_secs` を超えた場合は、LLM抽出を強制中断し、決定論的抽出結果へフォールバックします。

### 3. メモリ調停 (`arbiter.rs` - `MemoryArbiter`)
抽出された各 `MemoryCandidate` に対して、現在のデータベースの整合性を維持するための判定を実行します。
*   **`ArbiterReasonCode` (判定理由)**:
    -   `LowConfidence`: 確信度が設定された閾値（`min_confidence`）未満。
    -   `ExactDuplicate` / `SemanticDuplicate`: 内容または意味的ベクトルがすでに存在するため無視。
    -   `ContradictionSupersede`: 古い記憶と矛盾しており、新しい証拠の確信度が高いため古い方を `Superseded`（無効化）にして新規保存。
    -   `ContradictionDisputed`: 矛盾しているがどちらが正しいか判断がつかないため、既存記憶を `Disputed`（議論中）にマーク。
    -   `DeletionRequest`: ユーザーから「忘れて」と指示された記憶に合致するため `Archived` に遷移。
*   **判定アクション (`ArbiterAction`)**:
    最終的にデータベースに適用する `Persist`、`Ignore`、`Delete` などの物理的なコマンドを生成。

### 4. 自然忘却モデル (`forgetting.rs` - `ForgettingLifecycle`)
時間経過に伴う想起率の低下をモデル化。
*   時間経過による減衰スコア:
    $$\text{score} = \text{initial\_salience} \times e^{-\lambda t}$$
*   しきい値判定:
    -   想起スコアが `FADE_THRESHOLD` (0.3) を下回ると、ステータスを `Faded` に遷移。
    -   想起スコアが `ARCHIVE_THRESHOLD` (0.1) を下回ると、`Archived` に遷移し、通常の会話回想（MMR）の対象外とします。
*   ピン留め（`pinned = true`）された記憶は、時間経過による減衰計算の対象外（常にスコア1.0を維持）とします。
