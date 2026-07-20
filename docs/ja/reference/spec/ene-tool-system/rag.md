# ツールセマンティック RAG 検索仕様 (`ene-tool-rag`)

`ene-tool-rag` クレートは、ツール RAG (Retrieval-Augmented Generation) パイプラインを実装します。ユーザーのクエリ文脈に応じて、多数あるツールの中から最も適合する少数のツールを動的に選定し、システムプロンプトのトークン消費量を節約します。

---

## 1. データ構造

### `FieldWeights` (パブリック / 構造体)
ツールスペックの各メタデータフィールドに対するベクトル類似度スコアの重み値：
*   `summary`: 概要文（Summary）ベクトルの類似度重み（デフォルト: 1.0）。
*   `description`: 詳細な説明文（Description）の類似度重み（デフォルト: 0.6）。
*   `capability`: 表明されている機能説明（Capability）の類似度重み（デフォルト: 0.8）。
*   `example`: 実行コード例（Example）の類似度重み（デフォルト: 0.4）。
*   `negative`: **ネガティブマッチペナルティ**。ユーザーの操作が、該当ツールで定義されている「ネガティブキーワード」に合致した場合に適用される減点係数（デフォルト: -0.5）。

---

## 2. ツール選定および抽出プロセス (`rag.rs`)

#### `select`
*   **シグネチャ**: `pub async fn select(&self, query: &str) -> Vec<ToolSpec>`
*   **説明**: 指定されたユーザーの質問内容（クエリ）の埋め込みベクトルを計算し、条件に適合する `ToolSpec` 配列を返します。

#### `select_with_embedding`
*   **シグネチャ**: `pub async fn select_with_embedding(&self, query: &str, query_embedding: &[f32]) -> Vec<ToolSpec>`
*   **プロセス**:
    1.  データベースのストア接続がないテスト等の環境では、即座にバイパス対象（強制インジェクト）の基本ツールスペックのみを返します。
    2.  `MemoryStore::search_tools` を呼び出して、`tool_embedding_index` テーブルから類似するツール定義レコードを取得します。
    3.  検出されたレコードをツール識別名ごとにグループ化し、各フィールドのスコアに対して `FieldWeights` 補正比率を乗算して合計します。
    4.  算出した総合関連スコアが、検索の最小適合度閾値 `min_similarity`（デフォルト: 0.25）を下回るツール候補を完全に除外します。
    5.  残ったツール一覧に対し、カテゴリ上限数制限（`per_category_limits`）を適用し、特定のドメイン（ファイルツール等）だけで検索結果が埋め尽くされないように多様化トリミングを行います。
    6.  最終的なソート結果の上位 `final_n` 件（デフォルト: 6）のツールを、常にインジェクトが強制される基本ツール（`utility.question` など）とマージして返します。

#### `forced_only_specs`
*   **シグネチャ**: `fn forced_only_specs(&self) -> Vec<ToolSpec>`
*   **説明**: RAG フィルタリングの対象外として、常にプロンプトに含めるように事前定義されている必須基本ツールの定義を返します。

#### `stats`
*   **シグネチャ**: `pub async fn stats(&self) -> ToolRagStats`
*   **説明**: 現在インデックスされている全ツールの件数、およびベクトル件数データを含む統計オブジェクトを取得します。

---

## 3. インデックス作成およびキャッシュ更新

#### `ensure_index`
*   **シグネチャ**: `pub async fn ensure_index(&self, specs: &[ToolSpec], profiles: &[ToolRagProfile]) -> Result<(), EmbeddingError>`
*   **プロセス**:
    1.  現在登録されているツール一覧全体のハッシュ値を算出し（`compute_index_hash`）、前回のインデックス構築ハッシュと一致する場合は処理をスキップします。
    2.  データベースから、以前インデックスされたハッシュ情報の一覧を取得します。
    3.  すでに削除されたツールや、ハッシュ（定義スペック）に変更が生じている古いツールデータを `MemoryStore::delete_tool_embeddings` で一括削除します。
    4.  追加または変更されたツールの各フィールドについて、`index_field` を呼び出してベクトルを再計算・抽出し、データベースへ新規キャッシュ永続化します。

#### `index_field`
*   **シグネチャ**: `async fn index_field(&self, store: &Arc<MemoryStore>, cached: &HashMap<(String, String, String), (String, String)>, model_name: &str, profile: &ToolRagProfile, field: EmbeddingField, field_key: &str, example_index: Option<usize>, parameters: Option<&serde_json::Value>) -> Result<(), EmbeddingError>`
*   **説明**: 個別のツール定義テキストをロードし、埋め込みプロバイダを使用してベクトル化を実行して SQLite に保存します。

#### `start_background_indexer`
*   **シグネチャ**: `pub fn start_background_indexer(self: &Arc<Self>, specs: Vec<ToolSpec>, profiles: Vec<ToolRagProfile>)`
*   **説明**: アプリケーションの起動時や更新時の I/O 遅延を回避するため、インデックスの整合性チェックおよびキャッシュ再構築プロセスをバックグラウンドスレッドで起動します。

#### `field_version_hash`
*   **Signature**: `fn field_version_hash(field_name: &str, text: &str) -> String`
*   **Description**: 各カラムデータの内容ハッシュを算出して変更有無を監視します。

#### `is_cached`
*   **Signature**: `fn is_cached(cached: &HashMap<(String, String, String), (String, String)>, key: &(String, String, String), hash: &str, model: &str) -> bool`
*   **Description**: 特定のカラムデータがすでに正しいモデルバージョンでデータベース上に登録済みか確認します。

#### `persist`
*   **Signature**: `async fn persist(store: &Arc<MemoryStore>, tool_name: &str, field: &str, field_key: &str, version_hash: &str, model_name: &str, embedding: &[f32], source_text: &str) -> Result<(), EmbeddingError>`
*   **Description**: 生成された埋め込みベクトルとメタデータ情報をデータベースに書き込みます。

#### `compute_index_hash`
*   **Signature**: `fn compute_index_hash(specs: &[ToolSpec], profiles: &[ToolRagProfile]) -> u64`
*   **Description**: インデックス全体の一致ハッシュ値を算出します。
