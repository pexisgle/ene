# `TypedMemory` / 記憶分類および類似度スコアリング仕様

このドキュメントでは、Ene の長期記憶の分類種別（MemoryKind）、ステータス（MemoryStatus）、ライフサイクル遷移ルール、ハイブリッド検索候補の収集フロー、および記憶のスコアリング評価の数式モデルについて定義します。

---

## 1. 記憶の分類とステータス

### 1. 記憶の分類種別 (`MemoryKind` / 列挙型)
保存されるすべての記憶データは、以下のカテゴリのいずれか1つに分類されます：
*   `Episodic` (エピソード): 会話セグメントや発生した事実履歴。
*   `Semantic` (セマンティック): 一般的な知識や静的事実。
*   `UserProfile` (ユーザープロフィール): ユーザー自身に関する情報（名前、趣味など）。
*   `Relationship` (人間関係): ユーザーとアクターとの親密度や関係性の歴史。
*   `Affective` (感情状態): アクター自身の特定の感情のアンカー。
*   `Commitment` (約束): ユーザーと約束したタスクの内容。
*   `Preference` (嗜好): ユーザーの好き嫌いや興味関心事項。
*   `Procedure` (手順指示): ツールの操作ガイドやシステム手順。
*   `Reflection` (自己省察): アクター自身が過去の対話を振り返り自己分析した情報。

### 2. 記憶のステータス (`MemoryStatus` / 列挙型)
各記憶レコードのアクティブ状態および想起候補としての適格性を制御します：
*   `Active` (アクティブ): 最も適合度の高い、アクティブに想起可能なメモリ。
*   `Faded` (減衰): 時間経過により維持スコアが低下したメモリ。低い優先度で想起されます。
*   `Archived` (アーカイブ): バックアップ保管されたメモリ。RAG 想起の直接対象から除外されます。
*   `Disputed` (論争中): 事実関係に不一致がある状態。システムプロンプト内で不確実であると明記されます。
*   `Superseded` (上書き): より新しい事実によって更新され、無効化された古い情報。
*   `UserDeleted` (ユーザー削除): ユーザーからの削除命令により論理削除された状態。

---

## 2. シリアライズおよび値検証ヘルパー (`typed_memory.rs`)

#### `as_str` (for MemoryKind, MemoryStatus, MemoryScope, MemorySource)
*   **シグネチャ**: `pub const fn as_str(self) -> &'static str`
*   **説明**: 列挙型変数を、SQLite 保存用の静的な識別文字列定数にマッピングします。

#### `from_db_str` (for MemoryKind, MemoryStatus, MemoryScope, MemorySource)
*   **シグネチャ**: `pub(crate) fn from_db_str(s: &str) -> Self`
*   **説明**: SQLite からロードした文字列データを、アクター処理用の型定義にパース復元します。

#### `MemoryConfidence::new` / `MemorySalience::new`
*   **シグネチャ**: `pub fn new(raw: f32) -> Self`
*   **説明**: LLM が出力したメモリ抽出の信頼度（Confidence）および重要度（Salience）の値を `[0.0, 1.0]` の範囲内にクランプして初期化します。

#### `MemoryConfidence::get` / `MemorySalience::get`
*   **シグネチャ**: `pub const fn get(self) -> f32`
*   **説明**: 保存されているクランプ済みの float 値を返します。

---

## 3. 忘却ライフサイクルとステータス遷移 (`forgetting.rs`)

#### `user_restorable_statuses`
*   **シグネチャ**: `pub const fn user_restorable_statuses() -> &'static [MemoryStatus]`
*   **説明**: ユーザーの手動指示によって復旧・アクティブ化することが許可されているステータス（`Faded`、`Archived`、および `UserDeleted`）のリストを返します。

#### `validate_user_restore`
*   **シグネチャ**: `pub fn validate_user_restore(from: MemoryStatus) -> Result<(), InvalidTransition>`
*   **説明**: 元の状態が復旧可能なステータスリストに含まれているかを検証します。

#### `validate_transition`
*   **シグネチャ**: `pub const fn validate_transition(from: MemoryStatus, to: MemoryStatus) -> Result<(), InvalidTransition>`
*   **説明**: ステータス遷移マシンのルールを確認します（例: `Active` から `Faded` への移行は認められますが、新規データ書き込みなしに直接 `Superseded` にジャンプすることは拒否されます）。

#### `emotional_impact`
*   **シグネチャ**: `pub fn emotional_impact(affect: AffectAnnotation) -> f32`
*   **説明**: メモリに書き込まれている感情座標（PAD）データの合計分散値を算出し、記憶のインパクト重要度をスコアリングします。

#### `active_decay_anchor` / `faded_decay_anchor`
*   **シグネチャ**: `pub fn active_decay_anchor(item: &MemoryItem) -> DateTime<Utc>` (および Faded 版)
*   **説明**: 自然減衰計算の起点となる日時（最後に想起アクセスされた日時、または減衰状態に変更された日時）を決定します。

#### `decay_score`
*   **シグネチャ**: `pub fn decay_score(item: &MemoryItem, now: DateTime<Utc>, half_life_days: f64) -> f32`
*   **説明**: アクセス経過時間と忘却半減期設定から、メモリの減衰維持スコアを計算します。ピン留め（Pinned）されているメモリは常に減衰せず `1.0` を維持します。

#### `target_status_after_decay`
*   **シグネチャ**: `pub fn target_status_after_decay(current: MemoryStatus, score: f32) -> Option<MemoryStatus>`
*   **説明**: 維持スコアに応じて遷移ステータスを決定します（`0.3` 未満の場合は `Faded`、`0.1` 未満の場合は `Archived` への変更シグナルを生成）。

---

## 4. 検索スコアリング評価および類似度計算 (`search.rs`)

#### `tokenize`
*   **Signature**: `pub(crate) fn tokenize(text: &str) -> HashSet<String>`
*   **Description**: 比較用に文字列トークンを小文字ワードに分解します。

#### `document_lexical_similarity`
*   **シグネチャ**: `pub fn document_lexical_similarity(title_a: &str, content_a: &str, title_b: &str, content_b: &str) -> f32`
*   **説明**: 2つのテキストコンテンツ間の単語重複比率を Jaccard 類似度インデックスを用いて算出します。

#### `lexical_overlap_score`
*   **Signature**: `pub(crate) fn lexical_overlap_score(query: &str, title: &str, content: &str) -> f32`
*   **Description**: ユーザー入力クエリの単語群と、メモリレコードのタイトル・本文との一致率を測定します。

#### `recency_score`
*   **Signature**: `pub(crate) fn recency_score(reference: DateTime<Utc>, item: &MemoryItem, half_life_days: f64) -> f32`
*   **Description**: アクセス経過日時が最新のものほど想起されやすくなるよう、時間的リセンシースコアを算出します。

#### `emotional_match_score`
*   **Signature**: `pub(crate) fn emotional_match_score(query_affect: Option<AffectAnnotation>, item_affect: AffectAnnotation) -> f32`
*   **Description**: クエリ感情アノテーションと、メモリにバインドされた感情情報（PAD）の空間類似度を測定します。

#### `relationship_score`
*   **Signature**: `pub(crate) fn relationship_score(impact: f32) -> f32`
*   **Description**: 親愛度等に関係するメモリの重要度比率を補正します。

#### `access_boost_score`
*   **Signature**: `pub(crate) fn access_boost_score(access_count: i64) -> f32`
*   **Description**: 過去に頻繁にアクセス（想起）されたメモリに対して累積加算ボーナス（$0.02 \times \text{access\_count}$、最大 0.2）を算出します。

#### `contradiction_penalty` / `stale_penalty`
*   **Signature**: `pub(crate) fn contradiction_penalty(status: MemoryStatus) -> f32` (および Stale ペナルティ)
*   **Description**: ステータスが `Disputed`（論争中）または `Faded`（減衰）のドキュメントに対して、想起スコアのマイナス補正ペナルティを課します。

#### `is_recallable_status`
*   **Signature**: `pub(crate) const fn is_recallable_status(status: MemoryStatus) -> bool`
*   **Description**: ステータスが想起対象として認められる種類であるか検証します。

#### `score_candidate`
*   **シグネチャ**: `pub(crate) fn score_candidate(options: &Query<'_>, candidate: &GatheredCandidate) -> MemoryScoreBreakdown`
*   **説明**: ハイブリッド検索で収集されたメモリ候補に対し、以下の総合評価式に基づいて適合スコアを決定します：

$$\text{Total Score} = W_v S_v + W_l S_l + W_r S_r + W_s S_s + W_c S_c + W_e S_e + W_{\text{affinity}} S_{\text{affinity}} + B_a + B_{\text{cmt}} - P_{\text{dispute}} - P_{\text{stale}}$$

（$S_v$: ベクトルコサイン類似度、$S_l$: 語彙 Jaccard 重複率、$S_r$: リセンシー時間減衰、$S_s$: 重要度、$S_c$: 確信度、$S_e$: 感情 PAD 適合スコア、$S_{\text{affinity}}$: 親愛度補正、$B_a$: アクセス頻度ボーナス、$B_{\text{cmt}}$: 約束タスク優先ボーナス、$P_{\text{dispute}}$: 論争中ペナルティ、$P_{\text{stale}}$: 減衰ステータスペナルティ）。
