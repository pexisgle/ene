//! Tool RAG pipeline — multi-vector embedding, field-weighted similarity,
//! optional HyDE, optional LLM rerank, and per-category limits.
//!
//! Replaces the inline RAG logic previously embedded in
//! [`CompositeToolRegistry`](crate::CompositeToolRegistry).

use ene_memory::MemoryStore;
use ene_provider::{EmbeddingError, EmbeddingProvider, cosine_similarity};
use ene_tool_proto::{EmbeddingField, ToolName, ToolSpec};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::sync::atomic::{AtomicU64, Ordering};

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
    /// Weight for the HyDE (hypothetical document embedding).
    pub hyde: f32,
    /// Weight for the HyDE blend factor — the fraction
    /// of the final score contributed by the HyDE
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

impl From<super::config::FieldWeightsConfig> for FieldWeights {
    fn from(c: super::config::FieldWeightsConfig) -> Self {
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

/// Runtime options for the ToolRag pipeline, resolved from
/// [`super::config::ToolRagConfig`].
#[derive(Debug, Clone)]
pub struct ToolRagOptions {
    /// Whether the RAG pipeline is enabled.
    pub enabled: bool,
    /// Number of top candidates to retrieve from vector search.
    pub top_k: usize,
    /// Number of final tools to return after reranking.
    pub final_n: usize,
    /// Whether to use HyDE (hypothetical document embeddings).
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
            background_index_on_startup: false,
            forced: vec![
                ToolName::new("utility.question"),
                ToolName::new("utility.todo_add"),
                ToolName::new("utility.get_current_time"),
            ],
            weights: FieldWeights::default(),
        }
    }
}

impl TryFrom<super::config::ToolRagConfig> for ToolRagOptions {
    type Error = String;

    fn try_from(c: super::config::ToolRagConfig) -> Result<Self, Self::Error> {
        let mut forced = Vec::with_capacity(c.forced.len());
        for name in c.forced {
            // `forced` is config-driven (settings.json /
            // ENE_TOOL_RAG__FORCED__...) and therefore
            // untrusted. Reject invalid names with a
            // structured error so a typo / hostile config
            // entry surfaces as `Err`, not `panic!` during
            // startup.
            forced.push(ToolName::try_new(name)?);
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

/// The Tool RAG pipeline: embed → HyDE → weighted field similarity →
/// optional rerank → top-N.
pub struct ToolRag {
    embedder: Arc<dyn EmbeddingProvider>,
    store: Option<Arc<MemoryStore>>,
    opts: ToolRagOptions,
    /// Last-known ToolSpecs, used when returning results from [`select`].
    specs: RwLock<HashMap<ToolName, ToolSpec>>,
    /// Blake3 hash of the last indexed specs set.
    /// Used to fast-path ensure_index when specs haven't changed.
    last_specs_hash: AtomicU64,
    /// In-memory cache of tool embedding vectors.
    /// Populated by ensure_index, used by select. Avoids
    /// deserializing all f32 vecs from SQLite every turn.
    #[allow(clippy::type_complexity)]
    cached_field_rows: RwLock<Vec<(String, String, String, Vec<f32>)>>,
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
        config: super::config::ToolRagConfig,
    ) -> Result<Self, String> {
        let opts = ToolRagOptions::try_from(config)?;
        Ok(Self::new(embedder, store, opts))
    }

    /// Returns a reference to the runtime options.
    pub fn opts(&self) -> &ToolRagOptions {
        &self.opts
    }

    /// Whether the pipeline has a backing memory store.
    pub fn has_store(&self) -> bool {
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
            let cache = self.cached_field_rows.read().unwrap_or_else(|e| e.into_inner());
            if prev_hash == specs_hash && !cache.is_empty() {
                let mut map = self.specs.write().unwrap_or_else(|e| e.into_inner());
                map.clear();
                for spec in specs {
                    map.insert(spec.name.clone(), spec.clone());
                }
                return Ok(());
            }
        }

        let store = match &self.store {
            Some(s) => Arc::clone(s),
            None => {
                // Still cache specs even without a store.
                let mut map = self.specs.write().unwrap_or_else(|e| e.into_inner());
                map.clear();
                for spec in specs {
                    map.insert(spec.name.clone(), spec.clone());
                }
                return Ok(());
            }
        };

        // Load existing hashes from DB. We use the
        // hashes-only query here — the previous form
        // deserialized every f32 vector on every turn
        // and then discarded them. The vectors are
        // reloaded (still once per turn, in `select`)
        // only when needed for the actual ranking.
        let cached = match store.list_tool_embedding_hashes().await {
            Ok(entries) => {
                // Key: (tool_name, field, field_key) → (version_hash, model_name)
                let mut map: HashMap<(String, String, String), (String, String)> = HashMap::new();
                for (name, field, fkey, hash, model) in entries {
                    map.insert((name, field, fkey), (hash, model));
                }
                map
            }
            Err(e) => {
                tracing::warn!("[ToolRag] Failed to list cached embeddings: {}", e);
                HashMap::new()
            }
        };

        let model_name = self.embedder.model_name().to_string();
        let mut indexed = 0usize;
        let mut reused = 0usize;

        // ── Embed ToolSpec fields ──────────────────────────────────────
        for spec in specs {
            // (1) Summary
            let summary_text = spec.embedding_text(EmbeddingField::Summary);
            if !summary_text.is_empty() {
                let key = (
                    spec.name.as_str().to_string(),
                    "summary".into(),
                    String::new(),
                );
                let hash = field_version_hash("summary", &summary_text);
                if !is_cached(&cached, &key, &hash, &model_name) {
                    let emb = self
                        .embedder
                        .embed(&summary_text, ene_provider::EmbeddingKind::Summary)
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
                    indexed += 1;
                } else {
                    reused += 1;
                }
            }

            // (2) Description
            let desc_text = spec.embedding_text(EmbeddingField::Description);
            if !desc_text.is_empty() {
                let key = (
                    spec.name.as_str().to_string(),
                    "description".into(),
                    String::new(),
                );
                let hash = field_version_hash("description", &desc_text);
                if !is_cached(&cached, &key, &hash, &model_name) {
                    let emb = self
                        .embedder
                        .embed(&desc_text, ene_provider::EmbeddingKind::Description)
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
                    indexed += 1;
                } else {
                    reused += 1;
                }
            }

            // (3) Negative
            let neg_text = spec.embedding_text(EmbeddingField::Negative);
            if !neg_text.is_empty() {
                let key = (
                    spec.name.as_str().to_string(),
                    "negative".into(),
                    String::new(),
                );
                let hash = field_version_hash("negative", &neg_text);
                if !is_cached(&cached, &key, &hash, &model_name) {
                    let emb = self
                        .embedder
                        .embed(&neg_text, ene_provider::EmbeddingKind::Negative)
                        .await?;
                    persist(
                        &store,
                        spec.name.as_str(),
                        "negative",
                        "",
                        &hash,
                        &model_name,
                        &emb,
                        &neg_text,
                    )
                    .await?;
                    indexed += 1;
                } else {
                    reused += 1;
                }
            }

            // (4) Examples (one per example)
            for (i, example) in spec.examples.iter().enumerate() {
                let ex_text = format!("{}: {}", example.description, example.input);
                let field_key = format!("ex_{}", i);
                let key = (
                    spec.name.as_str().to_string(),
                    "example".into(),
                    field_key.clone(),
                );
                let hash = field_version_hash("example", &ex_text);
                if !is_cached(&cached, &key, &hash, &model_name) {
                    let emb = self
                        .embedder
                        .embed(&ex_text, ene_provider::EmbeddingKind::Example)
                        .await?;
                    persist(
                        &store,
                        spec.name.as_str(),
                        "example",
                        &field_key,
                        &hash,
                        &model_name,
                        &emb,
                        &ex_text,
                    )
                    .await?;
                    indexed += 1;
                } else {
                    reused += 1;
                }
            }
        }

        // Update cached spec map.
        {
            let mut map = self.specs.write().unwrap_or_else(|e| e.into_inner());
            map.clear();
            for spec in specs {
                map.insert(spec.name.clone(), spec.clone());
            }
        }

        // Cache all tool embeddings in memory
        match store.list_tool_embedding_fields().await {
            Ok(rows) => {
                let mapped: Vec<(String, String, String, Vec<f32>)> = rows
                    .into_iter()
                    .map(|(name, field, fkey, _hash, _model, emb, _src)| (name, field, fkey, emb))
                    .collect();
                let mut cache_write = self.cached_field_rows.write().unwrap_or_else(|e| e.into_inner());
                *cache_write = mapped;
            }
            Err(e) => {
                tracing::warn!("[ToolRag] Failed to cache tool embeddings after index build: {}", e);
            }
        }

        self.last_specs_hash.store(specs_hash, Ordering::Release);

        tracing::debug!("[ToolRag] Indexed {} fields, {} reused", indexed, reused);
        Ok(())
    }

    // ── Selection ──────────────────────────────────────────────────────

    /// Select the most relevant tools for the given query.
    ///
    /// Pipeline: embed query → optional HyDE → per-tool weighted field
    /// similarity → hard filters → optional rerank → top-N + forced tools.
    pub async fn select(&self, query: &str) -> Vec<ToolSpec> {
        let query_vec = match self.embedder.embed_query(query).await {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!("[ToolRag] Query embedding failed: {}", e);
                let map = self.specs.read().unwrap_or_else(|e| e.into_inner());
                return map.values().cloned().collect();
            }
        };

        self.select_with_embedding(query, &query_vec).await
    }

    /// Select the most relevant tools using a pre-computed query embedding.
    pub async fn select_with_embedding(&self, query: &str, query_embedding: &[f32]) -> Vec<ToolSpec> {
        let store = match &self.store {
            Some(s) => s,
            None => {
                // No store — return all known specs.
                let map = self.specs.read().unwrap_or_else(|e| e.into_inner());
                return map.values().cloned().collect();
            }
        };

        let query_vec = query_embedding.to_vec();

        // 2. Optional HyDE — generate a hypothetical document embedding.
        let hyde_vec = if self.opts.use_hyde {
            match self.embedder.hyde(query).await {
                Ok(hyde_text) => {
                    // Fast path: if the provider's hyde implementation just echoes the query
                    // (e.g. the default local embedding provider), we can reuse the existing
                    // query_embedding rather than running a second expensive inference pass.
                    if hyde_text == query {
                        Some(query_vec.clone())
                    } else {
                        self.embedder
                            .embed(&hyde_text, ene_provider::EmbeddingKind::Hyde)
                            .await
                            .ok()
                    }
                }
                Err(_) => None,
            }
        } else {
            None
        };

        // 3. Load all tool embeddings (from cache if available)
        let cached_rows = {
            let cache = self.cached_field_rows.read().unwrap_or_else(|e| e.into_inner());
            if !cache.is_empty() {
                Some(cache.clone())
            } else {
                None
            }
        };

        let field_rows: Vec<(String, String, String, Vec<f32>)> = match cached_rows {
            Some(rows) => rows,
            None => {
                match store.list_tool_embedding_fields().await {
                    Ok(rows) => {
                        let mapped: Vec<(String, String, String, Vec<f32>)> = rows
                            .into_iter()
                            .map(|(name, field, fkey, _hash, _model, emb, _src)| (name, field, fkey, emb))
                            .collect();
                        let mut cache_write = self.cached_field_rows.write().unwrap_or_else(|e| e.into_inner());
                        *cache_write = mapped.clone();
                        mapped
                    }
                    Err(e) => {
                        tracing::warn!("[ToolRag] Could not load embeddings: {}", e);
                        Vec::new()
                    }
                }
            }
        };

        // 4. Group by tool, compute weighted similarity.
        let w = &self.opts.weights;
        let mut per_tool: HashMap<String, (f32, Vec<FieldScore>)> = HashMap::new();

        for (tool_name, field, _fkey, emb) in &field_rows {
            let sim = cosine_similarity(&query_vec, emb);
            // Blend with HyDE embedding if available.
            // The HyDE blend factor is configurable
            // via `weights.hyde_blend` (replaces the
            // previously hardcoded 0.6 factor that was
            // not present in the documented formula).
            let blended = if let Some(ref hv) = hyde_vec {
                let hyde_sim = cosine_similarity(hv, emb);
                let blend = w.hyde_blend.clamp(0.0, 1.0);
                (sim * (1.0 - blend)) + (hyde_sim * w.hyde * blend)
            } else {
                sim
            };

            let weight = match field.as_str() {
                "summary" => w.summary,
                "description" => w.description,
                "example" => w.example,
                "negative" => w.negative,
                "hyde" => w.hyde,
                _ => 1.0,
            };

            let weighted = blended * weight;
            let entry = per_tool
                .entry(tool_name.clone())
                .or_insert((0.0, Vec::new()));
            entry.0 += weighted;
            entry.1.push(FieldScore {
                field: field.clone(),
                similarity: sim,
                weighted,
            });
        }

        // 5. Sort, filter, and apply category limits.
        let mut scored: Vec<(String, f32, Vec<FieldScore>)> = per_tool
            .into_iter()
            .map(|(name, (score, fields))| (name, score, fields))
            .filter(|(_, s, _)| *s >= self.opts.min_similarity)
            .collect();

        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        // Extract candidate ToolSpecs for reranking.
        let all_specs = {
            let map = self.specs.read().unwrap_or_else(|e| e.into_inner());
            map.clone()
        };

        let mut candidates: Vec<(ToolSpec, f32)> =
            Vec::with_capacity(scored.len().min(self.opts.rerank_candidates));
        for (name, score, _fields) in scored.iter().take(self.opts.rerank_candidates) {
            // `name` comes from the local RAG index, which
            // is populated only from validated `ToolSpec::name`
            // values. `try_new` here is a defensive belt-
            // and-suspenders check that surfaces a malformed
            // entry (e.g. a future DB row that was inserted
            // without validation) as a logged warning rather
            // than a panic.
            match ToolName::try_new(name.clone()) {
                Ok(tn) => {
                    if let Some(spec) = all_specs.get(&tn) {
                        candidates.push((spec.clone(), *score));
                    }
                }
                Err(e) => tracing::warn!("[ToolRag] Skipping invalid tool name in RAG index: {e}"),
            }
        }

        // 6. Optional rerank.
        if self.opts.use_rerank && candidates.len() > 1 {
            let rerank_specs: Vec<ToolSpec> = candidates.iter().map(|(s, _)| s.clone()).collect();
            match self.embedder.rerank(query, &rerank_specs).await {
                Ok(rerank_scores) => {
                    for (i, score) in rerank_scores.iter().enumerate() {
                        if i < candidates.len() {
                            candidates[i].1 = *score;
                        }
                    }
                    // Re-sort.
                    candidates
                        .sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
                }
                Err(e) => {
                    tracing::debug!("[ToolRag] Rerank failed, using embedding scores: {}", e);
                }
            }
        }

        // 7. Take the top `final_n` candidates. (Per-category
        //    limiting was previously wired through a
        //    per_category_limits option that nothing
        //    populated; the field has been removed.)
        let mut result: Vec<ToolSpec> = Vec::with_capacity(self.opts.final_n);

        for (spec, _score) in candidates.iter() {
            if result.len() >= self.opts.final_n {
                break;
            }
            result.push(spec.clone());
        }

        // 8. Union with forced_tools (no duplicates, prepended).
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
                Err(e) => tracing::warn!("[ToolRag] Background indexer failed: {}", e),
            }
        });
    }

    // ── Debug ──────────────────────────────────────────────────────────

    /// Returns a per-query ToolRagStats summary. The
    /// returned struct matches the public API documented
    /// in docs/api/ene-tool-host.md (`hits`, `total`,
    /// `top_similarity`).
    ///
    /// `hits` is the number of tools that would be
    /// returned by the configured `final_n`; `total`
    /// is the number of distinct tools in the index;
    /// `top_similarity` is the cosine similarity of the
    /// best match. When no store is attached (e.g. the
    /// in-memory fallback used by the CLI), all three
    /// are zero.
    pub async fn stats(&self) -> ToolRagStats {
        let store = match &self.store {
            Some(s) => s,
            None => {
                return ToolRagStats {
                    hits: 0,
                    total: 0,
                    top_similarity: 0.0,
                };
            }
        };

        let hashes = store.list_tool_embedding_hashes().await.unwrap_or_default();
        let mut total: usize = 0;
        let mut seen: HashMap<String, ()> = HashMap::new();
        for (name, _field, _fkey, _hash, _model) in hashes {
            if seen.insert(name.clone(), ()).is_none() {
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

#[derive(Debug, Clone, Default)]
#[expect(dead_code)]
struct FieldScore {
    field: String,
    similarity: f32,
    weighted: f32,
}

/// Compute a content-based version hash for a single field.
/// Uses blake3 (same algorithm as [`compute_tool_version_hash`](super::compute_tool_version_hash))
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
/// Observability summary of a Tool RAG select pass.
/// Matches the public API documented in
/// `docs/api/ene-tool-host.md` (`hits`, `total`,
/// `top_similarity`).
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
    u64::from_le_bytes(bytes[0..8].try_into().unwrap())
}
