# ene-tool-rag — Tool RAG パイプライン

利用可能なツール数が LLM のコンテキスト予算を超える場合、`ToolRag` が retrieval-augmented 選択を実行します: 埋め込み → 重み付きマルチフィールド類似度 → embedding cosine rerank → 上位 N 件。

**依存**: `ene-ai`（埋め込みプロバイダ）、`ene-store`（永続ツール埋め込みストレージ）、`ene-tool-proto`（ワイヤ型）、`ene-config`。

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
| `new` | `pub fn new(embedder: Arc<dyn EmbeddingProvider>, store: Option<Arc<MemoryStore>>, opts: ToolRagOptions) -> Self` | 解決済み `ToolRagOptions` があるときの直接コンストラクタ。 |
| `from_config` | `pub fn from_config(embedder: Arc<dyn EmbeddingProvider>, store: Option<Arc<MemoryStore>>, config: ToolRagConfig) -> Result<Self, ToolRagError>` | `ToolRagConfig` から `ToolRagOptions` を構築（`forced` の `Vec<String>` → `Vec<ToolName>` 変換を含む）し、パイプラインを構築する。 |
| `ensure_index` | `pub async fn ensure_index(&self, specs: &[ToolSpec], profiles: &[ToolRagProfile]) -> Result<(), EmbeddingError>` | specs + profiles の BLAKE3 ハッシュを計算。前回から変化がなければ高速 no-op。変化があれば各 `ToolRagProfile` のフィールドベクトル（`summary`, `description`, `capability`, `example`, `negative`）を（再）埋め込み・保存する。 |
| `select` | `pub async fn select(&self, query: &str) -> Vec<ToolSpec>` | 内部で `query` を埋め込み、`select_with_embedding` に委譲する。 |
| `select_with_embedding` | `pub async fn select_with_embedding(&self, query: &str, query_embedding: &[f32]) -> Vec<ToolSpec>` | `query_embedding` を使った重み付きフィールド類似度スコアリング、カテゴリ別上限、`top_k` 切り詰め、候補が複数ある場合の embedding cosine rerank を実行し、`opts.min_similarity` 以上の上位 `opts.final_n` ツールと `opts.forced` を返す。 |
| `start_background_indexer` | `pub fn start_background_indexer(self: &Arc<Self>, specs: Vec<ToolSpec>, profiles: Vec<ToolRagProfile>)` | `ensure_index` でキャッシュを温めるバックグラウンドタスクを spawn し、即座に戻る。 |
| `stats` | `pub async fn stats(&self) -> ToolRagStats` | 直近の `select` / `select_with_embedding` のスナップショット（ヒット数、インデックスサイズ、最高類似度）。 |
| `opts` | `pub fn opts(&self) -> &ToolRagOptions` | 解決済みオプションを返す。 |
| `has_store` | `pub fn has_store(&self) -> bool` | バックエンド `MemoryStore` が付いているか（再起動後も埋め込みを保持するには必要）。 |

---

## `ToolRagOptions`

```rust
#[derive(Debug, Clone)]
pub struct ToolRagOptions {
    pub top_k: usize,
    pub final_n: usize,
    pub rerank_candidates: usize,
    pub min_similarity: f32,
    pub forced: Vec<ToolName>,
    pub weights: FieldWeights,
    pub per_category_limits: HashMap<String, usize>,
}
```

`TryFrom<ToolRagConfig>` を実装。forced ツール名が無効な `ToolName` の場合はエラー。

---

## `FieldWeights`

各埋め込みフィールドがツール関連度スコアに与える重み。`negative` のような負の重みはハード除外ではなくソフトペナルティとして働く。

```rust
#[derive(Debug, Clone)]
pub struct FieldWeights {
    pub summary: f32,
    pub description: f32,
    pub capability: f32,
    pub example: f32,
    pub negative: f32,
}
```

シリアライズ可能な設定型からの変換に `From<FieldWeightsConfig>` を実装。

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

`ToolRagConfig` は `settings.json` の `tools.rag` にシリアライズされます（[設定](../configuration/settings.md) を参照）。

### `ToolRagConfig`

```rust
pub struct ToolRagConfig {
    pub enabled: bool,
    pub top_k: usize,
    pub final_n: usize,
    pub use_hyde: bool,       // 予約済み。LLM HyDE は無効（no-op）
    pub use_rerank: bool,     // true のとき cosine 埋め込みリランク
    pub rerank_candidates: usize,
    pub min_similarity: f32,
    pub background_index_on_startup: bool,
    pub forced: Vec<String>,
    pub weights: FieldWeightsConfig,
    pub per_category_limits: HashMap<String, usize>,
}
```

### `FieldWeightsConfig`

```rust
pub struct FieldWeightsConfig {
    pub summary: f32,
    pub description: f32,
    pub capability: f32,
    pub example: f32,
    pub negative: f32,
    pub hyde: f32,
    pub hyde_blend: f32,
}
```

`FieldWeights` のシリアライズ可能な対応型。`impl From<FieldWeightsConfig> for FieldWeights` で相互変換。

---

## エラー: `ToolRagError`

```rust
#[derive(Debug, Error)]
pub enum ToolRagError {
    #[error("Tool RAG configuration error: {message}")]
    Config { message: String },
}
```

`ToolRag::from_config` / `ToolRagOptions::from_config_lenient` は不正な forced 名をスキップし、パイプライン全体は失敗させません。

---

## 使用例

`ene-runtime` が `EneHandle::open` 時に、ツールと embedder が利用可能な場合に構築する:

```rust
let rag_config = ToolRagConfig::default();
let opts = ToolRagOptions::try_from(rag_config)?;
let rag = Arc::new(ToolRag::new(embedder.clone(), store, opts));
```

ストリーミングエンジンが LLM 推論前に関連ツールを選ぶ:

```rust
let tools = match &tool_rag {
    Some(rag) => rag.select(user_input).await,
    None => registry.list_tools(),
};
```

---

## 関連

- [`ene-tool-host`](./ene-tool-host.md) — ツールプロセスライフサイクル管理（RAG 利用側）
- [`ene-ai`](./ene-ai.md) — パイプラインで使う埋め込みプロバイダ
- [`ene-store`](./ene-store.md) — 永続埋め込みストレージ（`tool_embedding_index` テーブル）
- [`ene-tool-proto`](./ene-tool-proto.md) — `ToolSpec`, `ToolName`, `EmbeddingField` 型
- [`ene-config`](./ene-config.md) — 設定読み込み
- [ツールシステム概要](../tools/overview.md)
