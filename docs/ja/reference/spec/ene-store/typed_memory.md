# `TypedMemory` / 類似度探索クエリ & スコアリング仕様

本ドキュメントでは、記憶の種類・ライフサイクルステータス（Active / Faded / Archived 等）の定義、および SQLite（sqlite-vec）を用いたハイブリッド類似度検索（Vector + Lexical + Recency + Emotion）と、その詳細なスコアリング判定式の仕様を定義します。

---

## 1. 記憶分類とステータス定義

### 1. メモリ種別 (`MemoryKind` / 列挙型)
各長期記憶レコードは、以下のいずれかの種別を必ず保持します。
*   `Episodic`: エピソード記憶（いつ、何が起きたか）。
*   `Semantic`: 一般的な事実・知識（何が正しいか）。
*   `UserProfile`: ユーザーの属性、プロフィール、経歴。
*   `Relationship`: ユーザーとマスコットの関係性。
*   `Affective`: 感情的に強い印象を持つ記憶。
*   `Commitment`: マスコットがした約束やタスク。
*   `Preference`: ユーザーの好み・嫌悪。
*   `Procedure`: 操作手順、指示書。
*   `Reflection`: 会話履歴に対するマスコットの自己内省。

### 2. ライフサイクルステータス (`MemoryStatus` / 列挙型)
時間の経過（減衰）や上書き、削除に伴う記憶の状態。
*   `Active`: 有効な状態。通常の会話回想の対象。
*   `Faded`: やや薄れた状態。想起されにくくなりますが、検索候補には残ります。
*   `Archived`: 保管庫行き。通常の MMR 回想の対象外。
*   `Disputed`: 矛盾が検知され議論中の状態。プロンプト上では信頼性が低い事実としてマークされます。
*   `Superseded`: 新しい事実により上書きされ、無効化された状態。
*   `UserDeleted`: ユーザーの指示により明示的に削除された状態。

---

## 2. ハイブリッド検索仕様 (`MemoryStore::search`)

ユーザーのメッセージ入力時に、データベースから多角的な関連記憶をロードするため、以下の4つの経路で記憶候補を同時収集し、マージします。

1.  **ベクトル類似度探索 (`search_typed_memories_vector`)**:
    -   ユーザー入力の埋め込みベクトルを用い、`sqlite-vec` によるコサイン距離探索を実行。
    -   `1.0 - vec_distance_cosine(embedding, query)` にて上位候補をピックアップ。
2.  **全文キーワード（レキシカル）検索 (`list_lexical_typed_memory_candidates`)**:
    -   入力テキストに含まれるキーワードに基づき、SQLite の `LIKE` 句による部分一致検索を実行。
3.  **約束（コミットメント）の回収**:
    -   `list_active_commitments` により未完了タスクを取得し、紐づくメモリを最優先で回収。
4.  **直近記憶フォールバック**:
    -   類似する話題が見つからなかった場合の保険として、`list_recallable_typed_memories` から直近に作成・更新された記憶を一定件数（`recent_fallback_limit`）回収。

---

## 3. ハイブリッドスコア判定式

収集された全候補に対して、`score_candidate`（`search.rs`）により総合スコアを計算し、最も高い順にソートします。

### スコア算出の構成要素 (`MemoryScoreBreakdown`)

$$\text{Total Score} = W_v S_v + W_l S_l + W_r S_r + W_s S_s + W_c S_c + W_e S_e + W_{\text{affinity}} S_{\text{affinity}} + B_a + B_{\text{cmt}} - P_{\text{dispute}} - P_{\text{stale}}$$

*   **ベクトル類似度スコア ($S_v$)**: コサイン類似度値。
*   **レキシカル重複スコア ($S_l$)**: クエリ文字列とメモリの内容（Title/Content）の単語重複率（Jaccard係数等の簡易スコア）。
*   **時間減衰スコア ($S_r$)**:
    $$S_r = e^{-\lambda t}$$
    ここで $t$ は現在時刻からの経過日数。半減期（デフォルト30日）に従って減衰。
*   **Salienceスコア ($S_s$)**: 記憶作成時の重要度（LLMが判定した重み）。
*   **Confidenceスコア ($S_c$)**: 記憶の確信度（抽出時のLLMの自信度）。
*   **感情一致スコア ($S_e$)**: 記憶の感情タグ（Valence等）と、現在のマスコットの感情状態のユークリッド距離反比例値。
*   **親密度スコア ($S_{\text{affinity}}$)**: 親密度との相関。
*   **アクセス回数ブースト ($B_a$)**: 何回も呼び出された記憶（`access_count`）に対する加算（$0.02 \times \text{count}$、最大0.2）。
*   **コミットメントブースト ($B_{\text{cmt}}$)**: タスクに関係する記憶に対する加算。
*   **紛争状態ペナルティ ($P_{\text{dispute}}$)**: `Disputed` ステータスの場合は大きな減算。
*   **経年ペナルティ ($P_{\text{stale}}$)**: `Faded` ステータスによる減算。

算出された `total` スコアの上位から `limit` 件を抽出し、`ScoredMemory` として返却します。
