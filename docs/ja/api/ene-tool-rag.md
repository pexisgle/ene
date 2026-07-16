# ene-tool-rag — Tool RAG パイプライン

動的ツール選択のための Tool RAG（Retrieval-Augmented Generation）パイプライン。利用可能なツールの数がLLMのコンテキスト予算を超える場合、`ToolRag` は検索拡張生成（RAG）による選択ステップを実行します: 埋め込み → オプションのHyDE拡張 → 重み付き多フィールド類似度 → オプションのクロスエンコーダーによるリランク → 上位N件。

**依存関係**: `ene-ai`（埋め込みプロバイダー、HyDE、リランク）、`ene-store`（永続ツール埋め込みストレージ）、`ene-tool-proto`（ワイヤー型）、`ene-config`。

---

## `ToolRag`

```rust
pub struct ToolRag {
    embedder: Arc<dyn EmbeddingProvider>,
    store: Option<Arc<MemoryStore>>,
    opts: ToolRagOptions,
    specs: RwLock<HashMap<ToolName, ToolSpec>>,
    last_specs_hash: AtomicU64,
    cached_field_rows: RwLock<Vec<CachedFieldRow>>,
}
```

| メソッド | シグネチャ | 説明 |
|--------|-----------|-------------|
| `new` | `pub fn new(embedder: Arc<dyn EmbeddingProvider>, store: Option<Arc<MemoryStore>>, opts: ToolRagOptions) -> Self` | すでに解決済みの `ToolRagOptions` がある場合の直接コンストラクタ。 |
| `from_config` | `pub fn from_config(embedder: Arc<dyn EmbeddingProvider>, store: Option<Arc<MemoryStore>>, config: ToolRagConfig) -> Result<Self, ToolRagError>` | 設定向けの `ToolRagConfig` から `ToolRagOptions` を構築する（`forced` 用に `Vec<String>` → `Vec<ToolName>` へ変換）し、パイプラインを構築する。 |
| `ensure_index` | `pub async fn ensure_index(&self, specs: &[ToolSpec]) -> Result<(), EmbeddingError>` | 仕様セットに対してBLAKE3ハッシュを計算する。前回の呼び出しから変化がなければ高速なno-opになる。変化があれば、変更されたツールを（再）埋め込みし、フィールドごとのベクトルを保存する。 |
| `select` | `pub async fn select(&self, query: &str) -> Vec<ToolSpec>` | 内部で `query` を埋め込み、`select_with_embedding` に委譲する。 |
| `select_with_embedding` | `pub async fn select_with_embedding(&self, query: &str, query_embedding: &[f32]) -> Vec<ToolSpec>` | （`query_embedding` を使った）重み付きフィールド単位の類似度スコアリングに加え、オプションのHyDEブレンディングとリランクを実行し、`opts.min_similarity` を上回る上位 `opts.final_n` 件のツールを返す。`opts.forced` のツールは常に含まれる。 |
| `start_background_indexer` | `pub fn start_background_indexer(self: &Arc<Self>, specs: Vec<ToolSpec>)` | `ensure_index` を呼び出してキャッシュをウォームアップするバックグラウンドタスクを生成する。即座に処理を返す。 |
| `stats` | `pub async fn stats(&self) -> ToolRagStats` | 直前の `select`/`select_with_embedding` 呼び出しのスナップショット: ヒット数、インデックスサイズ、最高類似度。 |
| `opts` | `pub fn opts(&self) -> &ToolRagOptions` | 解決済みのオプションを返す。 |
| `has_store` | `pub fn has_store(&self) -> bool` | バックの `MemoryStore` が存在するかどうか（RAGは再起動後も埋め込みを永続化するために必要）。 |

---

## `ToolRagOptions`

```rust
#[derive(Debug, Clone)]
pub struct ToolRagOptions {
    pub enabled: bool,
    pub top_k: usize,
    pub final_n: usize,
    pub use_hyde: bool,
    pub use_rerank: bool,
    pub rerank_candidates: usize,
    pub min_similarity: f32,
    pub background_index_on_startup: bool,
    pub forced: Vec<ToolName>,
    pub weights: FieldWeights,
}
```

`TryFrom<ToolRagConfig>` を実装 — すべての強制ツール名が有効な `ToolName` であることを検証する。

---

## `FieldWeights`

各埋め込みフィールドがツールの関連性スコアにどれだけ寄与するかを制御する。マイナスの重み（例: `negative`）は、ハードな除外ではなくソフトなペナルティとして機能する。

```rust
#[derive(Debug, Clone)]
pub struct FieldWeights {
    pub summary: f32,
    pub description: f32,
    pub example: f32,
    pub negative: f32,
    pub hyde: f32,
    pub hyde_blend: f32,
}
```

シリアライズ可能な設定型からの変換用に `From<FieldWeightsConfig>` を実装する。

---

## `ToolRagStats`

```rust
#[derive(Debug, Clone, Default)]
pub struct ToolRagStats {
    pub hits: usize,
    pub total: usize,
    pub top_similarity: f32,
}
```

---

## 設定型

`settings.json` の `[tools.rag]` セクションに対応し、`HasConfigKey` トレイトを用いてパス `&["tools", "rag"]` で読み込まれる。

### `ToolRagConfig`

```rust
pub struct ToolRagConfig {
    pub enabled: bool,
    pub top_k: usize,
    pub final_n: usize,
    pub use_hyde: bool,
    pub use_rerank: bool,
    pub rerank_candidates: usize,
    pub min_similarity: f32,
    pub background_index_on_startup: bool,
    pub forced: Vec<String>,
    pub weights: FieldWeightsConfig,
}
```

### `FieldWeightsConfig`

```rust
pub struct FieldWeightsConfig {
    pub summary: f32,
    pub description: f32,
    pub example: f32,
    pub negative: f32,
    pub hyde: f32,
    pub hyde_blend: f32,
}
```

`FieldWeights` のシリアライズ可能なカウンターパート — `impl From<FieldWeightsConfig> for FieldWeights` で変換される。

---

## エラー: `ToolRagError`

```rust
#[derive(Debug, Error)]
pub enum ToolRagError {
    #[error("Tool RAG configuration error: {message}")]
    Config { message: String },
}
```

設定に無効な強制ツール名が含まれている場合に `ToolRagOptions::try_from` および `ToolRag::from_config` から返される。

---

## 使い方

`ene-runtime` が `EneHandle::open` 時に構築する:

```rust
let rag_config = config.get_section::<ToolRagConfig>().unwrap_or_default();
let opts = ToolRagOptions::try_from(rag_config)?;
let rag = Arc::new(ToolRag::new(embedder.clone(), store, opts));
```

ストリーミングエンジンがLLM推論の前に関連ツールを選択するために使用する:

```rust
let tools = match &tool_rag {
    Some(rag) => rag.select(user_input).await,
    None => registry.list_tools(),
};
```

---

## 関連ドキュメント

- [`ene-tool-host`](./ene-tool-host.md) — ツールプロセスライフサイクル管理（RAGのコンシューマ）
- [`ene-ai`](./ene-ai.md) — パイプラインが使用する埋め込み・リランクプロバイダー
- [`ene-store`](./ene-store.md) — 永続埋め込みストレージ（`tool_embedding_index` テーブル）
- [`ene-tool-proto`](./ene-tool-proto.md) — `ToolSpec`、`ToolName`、`EmbeddingField` 型
- [`ene-config`](./ene-config.md) — 設定読み込み（`ToolRagConfig`、`HasConfigKey`）
- [ツールシステム概要](../tools/overview.md)
