//! Tool RAG pipeline — multi-vector embedding, field-weighted similarity,
//! optional HyDE, optional LLM rerank, and per-category limits.

use ene_ai::{EmbeddingError, EmbeddingProvider, cosine_similarity, embed, embed_query};
use ene_store::MemoryStore;
use ene_tool_proto::types::EmbeddingField;
use ene_tool_proto::{ToolName, ToolSpec};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};

// ── Per-field similarity weights ──────────────────────────────────────────

/// Controls how strongly each embedding field contributes to the per-tool
/// relevance score. Negative weights (e.g. on `negative`) produce a soft
/// penalty rather than hard exclusion.
#[derive(Debug, Clone)]
pub struct FieldWeights {
    /// Weight for the tool summary embedding.
    pub summary: f32,
    /// Weight for the tool description embedding.
    pub description: f32,
    /// Weight for the tool example embedding.
    pub example: f32,
    /// Weight for the negative/unwanted embedding (penalizes matches).
    pub negative: f32,
    /// Weight for the `HyDE` (hypothetical document embedding).
    pub hyde: f32,
    /// Weight for the `HyDE` blend factor — the fraction
    /// of the final score contributed by the `HyDE`
    /// similarity, with the remainder from the direct
    /// per-field cosine similarity. Replaces the
    /// previously hardcoded 0.6 factor.
    pub hyde_blend: f32,
}

impl Default for FieldWeights {
    fn default() -> Self {
        Self {
            summary: 1.0,
            description: 0.6,
            example: 0.4,
            negative: -0.5,
            hyde: 0.7,
            hyde_blend: 0.6,
        }
    }
}

impl From<crate::config::FieldWeightsConfig> for FieldWeights {
    fn from(c: crate::config::FieldWeightsConfig) -> Self {
        Self {
            summary: c.summary,
            description: c.description,
            example: c.example,
            negative: c.negative,
            hyde: c.hyde,
            hyde_blend: c.hyde_blend,
        }
    }
}

// ── Runtime options (derived from config) ─────────────────────────────────

/// Runtime options for the `ToolRag` pipeline, resolved from
/// [`crate::config::ToolRagConfig`].
#[derive(Debug, Clone)]
pub struct ToolRagOptions {
    /// Whether the RAG pipeline is enabled.
    pub enabled: bool,
    /// Number of top candidates to retrieve from vector search.
    pub top_k: usize,
    /// Number of final tools to return after reranking.
    pub final_n: usize,
    /// Whether to use `HyDE` (hypothetical document embeddings).
    pub use_hyde: bool,
    /// Whether to rerank candidates with a cross-encoder.
    pub use_rerank: bool,
    /// Number of candidates to consider during reranking.
    pub rerank_candidates: usize,
    /// Minimum similarity score for a tool to be included.
    pub min_similarity: f32,
    /// Whether to index tools in the background on startup.
    pub background_index_on_startup: bool,
    /// Tools that are always included regardless of RAG scoring.
    pub forced: Vec<ToolName>,
    /// Per-field embedding weights used for scoring.
    pub weights: FieldWeights,
}

impl Default for ToolRagOptions {
    fn default() -> Self {
        Self {
            enabled: true,
            top_k: 12,
            final_n: 6,
            use_hyde: true,
            use_rerank: true,
            rerank_candidates: 24,
            min_similarity: 0.25,
            background_index_on_startup: true,
            forced: vec![
                ToolName::new("utility.question"),
                ToolName::new("utility.todo_add"),
                ToolName::new("utility.get_current_time"),
            ],
            weights: FieldWeights::default(),
        }
    }
}

impl TryFrom<crate::config::ToolRagConfig> for ToolRagOptions {
    type Error = crate::ToolRagError;

    fn try_from(c: crate::config::ToolRagConfig) -> Result<Self, Self::Error> {
        let mut forced = Vec::with_capacity(c.forced.len());
        for name in c.forced {
            forced.push(
                ToolName::try_new(name).map_err(|e| crate::ToolRagError::Config { message: e })?,
            );
        }
        Ok(Self {
            enabled: c.enabled,
            top_k: c.top_k,
            final_n: c.final_n,
            use_hyde: c.use_hyde,
            use_rerank: c.use_rerank,
            rerank_candidates: c.rerank_candidates,
            min_similarity: c.min_similarity,
            background_index_on_startup: c.background_index_on_startup,
            forced,
            weights: FieldWeights::from(c.weights),
        })
    }
}

// ── Pipeline ──────────────────────────────────────────────────────────────

/// The Tool RAG pipeline: embed → `HyDE` → weighted field similarity →
/// optional rerank → top-N.
pub struct ToolRag {
    embedder: Arc<dyn EmbeddingProvider>,
    store: Option<Arc<MemoryStore>>,
    opts: ToolRagOptions,
    /// Last-known `ToolSpecs`, used when returning results from [`select`].
    specs: RwLock<HashMap<ToolName, ToolSpec>>,
    /// Blake3 hash of the last indexed specs set.
    /// Used to fast-path `ensure_index` when specs haven't changed.
    last_specs_hash: AtomicU64,
    /// In-memory cache of tool embedding vectors.
    /// Populated by `ensure_index`, used by select. Avoids
    /// deserializing all f32 vecs from SQLite every turn.
    cached_field_rows: RwLock<Vec<CachedFieldRow>>,
}

impl ToolRag {
    /// Creates a new `ToolRag` instance with the given embedder, optional memory store, and options.
    pub fn new(
        embedder: Arc<dyn EmbeddingProvider>,
        store: Option<Arc<MemoryStore>>,
        opts: ToolRagOptions,
    ) -> Self {
        Self {
            embedder,
            store,
            opts,
            specs: RwLock::new(HashMap::new()),
            last_specs_hash: AtomicU64::new(0),
            cached_field_rows: RwLock::new(Vec::new()),
        }
    }

    /// Construct from a config-level `ToolRagConfig`.
    ///
    /// Returns an error if `config.forced` contains a tool
    /// name that does not satisfy [`ToolName::is_valid`]. The
    /// error originates from the untrusted config file, not
    /// from this crate.
    pub fn from_config(
        embedder: Arc<dyn EmbeddingProvider>,
        store: Option<Arc<MemoryStore>>,
        config: crate::config::ToolRagConfig,
    ) -> Result<Self, crate::ToolRagError> {
        let opts = ToolRagOptions::try_from(config)?;
        Ok(Self::new(embedder, store, opts))
    }

    /// Returns a reference to the runtime options.
    pub const fn opts(&self) -> &ToolRagOptions {
        &self.opts
    }

    /// Whether the pipeline has a backing memory store.
    pub const fn has_store(&self) -> bool {
        self.store.is_some()
    }

    // ── Indexing ───────────────────────────────────────────────────────

    /// Ensures every spec is embedded and stored.
    ///
    /// Only re-embeds fields whose text-derived version hash has changed.
    /// Called by the streaming engine before each [`select`](Self::select).
    pub async fn ensure_index(&self, specs: &[ToolSpec]) -> Result<(), EmbeddingError> {
        let specs_hash = compute_specs_hash(specs);
        let prev_hash = self.last_specs_hash.load(Ordering::Acquire);
        {
            let cache = self
                .cached_field_rows
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if prev_hash == specs_hash && !cache.is_empty() {
                let mut map = self
                    .specs
                    .write()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                map.clear();
                for spec in specs {
                    map.insert(spec.name.clone(), spec.clone());
                }
                drop(map);
                return Ok(());
            }
        }

        let store = if let Some(s) = &self.store {
            Arc::clone(s)
        } else {
            let mut map = self
                .specs
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            map.clear();
            for spec in specs {
                map.insert(spec.name.clone(), spec.clone());
            }
            drop(map);
            return Ok(());
        };

        let cached = match store.list_tool_embedding_hashes().await {
            Ok(entries) => {
                let mut map: HashMap<(String, String, String), (String, String)> = HashMap::new();
                for (name, field, fkey, hash, model) in entries {
                    map.insert((name, field, fkey), (hash, model));
                }
                map
            }
            Err(e) => {
                tracing::warn!(component = "ToolRag", error = %e, "Failed to list cached embeddings");
                HashMap::new()
            }
        };

        let model_name = self.embedder.model_name().to_string();

        for spec in specs {
            let summary_text = spec.embedding_text(EmbeddingField::Summary);
            if !summary_text.is_empty() {
                let key = (
                    spec.name.as_str().to_string(),
                    "summary".into(),
                    String::new(),
                );
                let hash = field_version_hash("summary", &summary_text);
                if !is_cached(&cached, &key, &hash, &model_name) {
                    let emb = embed(
                        self.embedder.as_ref(),
                        &summary_text,
                        ene_ai::EmbeddingKind::Summary,
                    )
                    .await?;
                    persist(
                        &store,
                        spec.name.as_str(),
                        "summary",
                        "",
                        &hash,
                        &model_name,
                        &emb,
                        &summary_text,
                    )
                    .await?;
                }
            }

            let desc_text = spec.embedding_text(EmbeddingField::Description);
            if !desc_text.is_empty() {
                let key = (
                    spec.name.as_str().to_string(),
                    "description".into(),
                    String::new(),
                );
                let hash = field_version_hash("description", &desc_text);
                if !is_cached(&cached, &key, &hash, &model_name) {
                    let emb = embed(
                        self.embedder.as_ref(),
                        &desc_text,
                        ene_ai::EmbeddingKind::Description,
                    )
                    .await?;
                    persist(
                        &store,
                        spec.name.as_str(),
                        "description",
                        "",
                        &hash,
                        &model_name,
                        &emb,
                        &desc_text,
                    )
                    .await?;
                }
            }
        }

        {
            let mut map = self
                .specs
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            map.clear();
            for spec in specs {
                map.insert(spec.name.clone(), spec.clone());
            }
        }

        match store.list_tool_embedding_fields().await {
            Ok(rows) => {
                let mapped: Vec<CachedFieldRow> = rows
                    .into_iter()
                    .map(
                        |(tool_name, field, _field_key, _hash, _model, embedding, _src)| {
                            CachedFieldRow {
                                tool_name,
                                field,
                                embedding,
                            }
                        },
                    )
                    .collect();
                let mut cache_write = self
                    .cached_field_rows
                    .write()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                *cache_write = mapped;
            }
            Err(e) => {
                tracing::warn!(
                    "[ToolRag] Failed to cache tool embeddings after index build: {}",
                    e
                );
            }
        }

        self.last_specs_hash.store(specs_hash, Ordering::Release);

        Ok(())
    }

    // ── Selection ──────────────────────────────────────────────────────

    /// Select the most relevant tools for the given query.
    ///
    /// Pipeline: embed query → optional `HyDE` → per-tool weighted field
    /// similarity → hard filters → optional rerank → top-N + forced tools.
    pub async fn select(&self, query: &str) -> Vec<ToolSpec> {
        let query_vec = match embed_query(self.embedder.as_ref(), query).await {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(component = "ToolRag", error = %e, "Query embedding failed");
                let map = self
                    .specs
                    .read()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                return map.values().cloned().collect();
            }
        };

        self.select_with_embedding(query, &query_vec).await
    }

    /// Select the most relevant tools using a pre-computed query embedding.
    pub async fn select_with_embedding(
        &self,
        query: &str,
        query_embedding: &[f32],
    ) -> Vec<ToolSpec> {
        let t_start = std::time::Instant::now();
        let Some(store) = &self.store else {
            let map = self
                .specs
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            return map.values().cloned().collect();
        };

        let query_vec = query_embedding.to_vec();

        let hyde_vec = if self.opts.use_hyde {
            match ene_ai::hyde_document(None, query).await {
                Ok(hyde_text) => {
                    if hyde_text == query {
                        Some(query_vec.clone())
                    } else {
                        ene_ai::embed(
                            self.embedder.as_ref(),
                            &hyde_text,
                            ene_ai::EmbeddingKind::Hyde,
                        )
                        .await
                        .ok()
                    }
                }
                Err(_) => None,
            }
        } else {
            None
        };
        let t_hyde = t_start.elapsed();

        let cached_rows = {
            let cache = self
                .cached_field_rows
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if cache.is_empty() {
                None
            } else {
                Some(cache.clone())
            }
        };

        let field_rows: Vec<CachedFieldRow> = match cached_rows {
            Some(rows) => rows,
            None => match store.list_tool_embedding_fields().await {
                Ok(rows) => {
                    let mapped: Vec<CachedFieldRow> = rows
                        .into_iter()
                        .map(
                            |(tool_name, field, _field_key, _hash, _model, embedding, _src)| {
                                CachedFieldRow {
                                    tool_name,
                                    field,
                                    embedding,
                                }
                            },
                        )
                        .collect();
                    let mut cache_write = self
                        .cached_field_rows
                        .write()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    cache_write.clone_from(&mapped);
                    mapped
                }
                Err(e) => {
                    tracing::warn!(component = "ToolRag", error = %e, "Could not load embeddings");
                    Vec::new()
                }
            },
        };
        let t_load = t_start.elapsed();

        let w = &self.opts.weights;
        let mut per_tool: HashMap<String, (f32, Vec<FieldScore>)> = HashMap::new();

        for row in &field_rows {
            let sim = cosine_similarity(&query_vec, &row.embedding);
            let blended = if let Some(ref hv) = hyde_vec {
                let hyde_sim = cosine_similarity(hv, &row.embedding);
                let blend = w.hyde_blend.clamp(0.0, 1.0);
                (hyde_sim * w.hyde).mul_add(blend, sim * (1.0 - blend))
            } else {
                sim
            };

            let weight = match row.field.as_str() {
                "summary" => w.summary,
                "description" => w.description,
                "example" => w.example,
                "negative" => w.negative,
                "hyde" => w.hyde,
                _ => 1.0,
            };

            let weighted = blended * weight;
            let entry = per_tool
                .entry(row.tool_name.clone())
                .or_insert((0.0, Vec::new()));
            entry.0 += weighted;
            entry.1.push(FieldScore {
                field: row.field.clone(),
                similarity: sim,
                weighted,
            });
        }

        let mut scored: Vec<(String, f32, Vec<FieldScore>)> = per_tool
            .into_iter()
            .map(|(name, (score, fields))| (name, score, fields))
            .filter(|(_, s, _)| *s >= self.opts.min_similarity)
            .collect();

        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        let all_specs = {
            let map = self
                .specs
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            map.clone()
        };

        let mut candidates: Vec<(ToolSpec, f32)> =
            Vec::with_capacity(scored.len().min(self.opts.rerank_candidates));
        for (name, score, _fields) in scored.iter().take(self.opts.rerank_candidates) {
            match ToolName::try_new(name.clone()) {
                Ok(tn) => {
                    if let Some(spec) = all_specs.get(&tn) {
                        candidates.push((spec.clone(), *score));
                    }
                }
                Err(e) => {
                    tracing::warn!(component = "ToolRag", error = %e, "Skipping invalid tool name in RAG index");
                }
            }
        }
        let t_score = t_start.elapsed();

        if self.opts.use_rerank && candidates.len() > 1 {
            let rerank_specs: Vec<ToolSpec> = candidates.iter().map(|(s, _)| s.clone()).collect();
            match ene_ai::rerank_tool_specs(self.embedder.as_ref(), None, query, &rerank_specs)
                .await
            {
                Ok(rerank_scores) => {
                    for (i, score) in rerank_scores.iter().enumerate() {
                        if i < candidates.len() {
                            candidates[i].1 = *score;
                        }
                    }
                    candidates
                        .sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
                }
                Err(e) => {
                    tracing::debug!(component = "ToolRag", error = %e, "Rerank failed, using embedding scores");
                }
            }
        }

        let mut result: Vec<ToolSpec> = Vec::with_capacity(self.opts.final_n);

        for (spec, _score) in &candidates {
            if result.len() >= self.opts.final_n {
                break;
            }
            result.push(spec.clone());
        }

        let t_rerank = t_start.elapsed();
        tracing::debug!(
            component = "ToolRag",
            "Timings: hyde={:?}, load={:?}, score={:?}, rerank={:?}",
            t_hyde,
            t_load.checked_sub(t_hyde).unwrap_or_default(),
            t_score.checked_sub(t_load).unwrap_or_default(),
            t_rerank.checked_sub(t_score).unwrap_or_default()
        );

        {
            let result_names: Vec<ToolName> = result.iter().map(|s| s.name.clone()).collect();
            for forced_name in self.opts.forced.iter().rev() {
                if !result_names.contains(forced_name)
                    && let Some(spec) = all_specs.get(forced_name)
                {
                    result.insert(0, spec.clone());
                }
            }
            result.truncate(self.opts.final_n + self.opts.forced.len());
        }

        result
    }

    // ── Background indexing ─────────────────────────────────────────────

    /// Spawns a background task that warms the index with the given specs.
    /// Returns immediately; the indexing runs asynchronously.
    pub fn start_background_indexer(self: &Arc<Self>, specs: Vec<ToolSpec>) {
        let rag = Arc::clone(self);
        tokio::spawn(async move {
            match rag.ensure_index(&specs).await {
                Ok(()) => tracing::info!(
                    "[ToolRag] Background indexer completed for {} specs",
                    specs.len()
                ),
                Err(e) => {
                    tracing::warn!(component = "ToolRag", error = %e, "Background indexer failed");
                }
            }
        });
    }

    // ── Debug ──────────────────────────────────────────────────────────

    /// Returns a per-query `ToolRagStats` summary.
    pub async fn stats(&self) -> ToolRagStats {
        let Some(store) = &self.store else {
            return ToolRagStats {
                hits: 0,
                total: 0,
                top_similarity: 0.0,
            };
        };

        let hashes = store.list_tool_embedding_hashes().await.unwrap_or_default();
        let mut total: usize = 0;
        let mut seen: HashSet<String> = HashSet::new();
        for (name, _field, _fkey, _hash, _model) in hashes {
            if seen.insert(name.clone()) {
                total += 1;
            }
        }

        ToolRagStats {
            hits: 0,
            total,
            top_similarity: 0.0,
        }
    }
}

// ── Helpers ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
struct CachedFieldRow {
    tool_name: String,
    field: String,
    embedding: Vec<f32>,
}

#[derive(Debug, Clone, Default)]
#[expect(dead_code)]
struct FieldScore {
    field: String,
    similarity: f32,
    weighted: f32,
}

/// Compute a content-based version hash for a single field.
/// Uses blake3 (same algorithm as `compute_tool_version_hash`)
/// so that embedding cache keys are consistent across the codebase.
fn field_version_hash(field_name: &str, text: &str) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(field_name.as_bytes());
    hasher.update(b"\0");
    hasher.update(text.as_bytes());
    hasher.finalize().to_hex().to_string()
}

/// Check whether a cached (hash, model) pair matches the current values.
fn is_cached(
    cached: &HashMap<(String, String, String), (String, String)>,
    key: &(String, String, String),
    hash: &str,
    model: &str,
) -> bool {
    match cached.get(key) {
        Some((cached_hash, cached_model)) => cached_hash == hash && cached_model == model,
        None => false,
    }
}

/// Persist a single field embedding.
async fn persist(
    store: &Arc<MemoryStore>,
    tool_name: &str,
    field: &str,
    field_key: &str,
    version_hash: &str,
    model_name: &str,
    embedding: &[f32],
    source_text: &str,
) -> Result<(), EmbeddingError> {
    store
        .upsert_tool_embedding_field(
            tool_name,
            field,
            field_key,
            version_hash,
            model_name,
            embedding,
            source_text,
        )
        .await
        .map_err(|e| EmbeddingError::Provider(e.to_string()))
}

// ── Stats ──────────────────────────────────────────────────────────────────

/// Snapshot returned by [`ToolRag::stats`].
#[derive(Debug, Clone, Default)]
pub struct ToolRagStats {
    /// Number of tools returned to the caller in the
    /// most recent `select` call.
    pub hits: usize,
    /// Total number of distinct tools in the index.
    pub total: usize,
    /// Cosine similarity of the best match in the
    /// most recent `select` call.
    pub top_similarity: f32,
}

fn compute_specs_hash(specs: &[ToolSpec]) -> u64 {
    let mut hasher = blake3::Hasher::new();
    for spec in specs {
        if let Ok(bytes) = serde_json::to_vec(spec) {
            hasher.update(&bytes);
        } else {
            hasher.update(spec.name.as_str().as_bytes());
        }
    }
    let hash = hasher.finalize();
    let bytes = hash.as_bytes();
    let mut array = [0u8; 8];
    array.copy_from_slice(&bytes[0..8]);
    u64::from_le_bytes(array)
}
