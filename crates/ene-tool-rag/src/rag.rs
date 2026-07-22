//! Tool RAG pipeline — multi-vector embedding, field-weighted similarity,
//! optional cosine rerank, and per-category limits.
//!
//! LLM `HyDE` is deprecated and disabled; `use_hyde` is retained as a no-op
//! config knob scheduled for removal.

use ene_ai::{EmbeddingError, EmbeddingProvider, cosine_similarity, embed, embed_query};
use ene_store::MemoryStore;
use ene_tool_proto::types::EmbeddingField;
use ene_tool_proto::{ToolName, ToolRagProfile, ToolSpec};
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
    /// Weight for the capability embedding.
    pub capability: f32,
    /// Weight for the tool example embedding.
    pub example: f32,
    /// Weight for the negative/unwanted embedding (penalizes matches).
    pub negative: f32,
    /// Weight for the `HyDE` (hypothetical document embedding).
    ///
    /// Deprecated: LLM `HyDE` is disabled; this weight is unused and scheduled for removal.
    #[deprecated(note = "LLM HyDE is disabled; this weight is unused and scheduled for removal")]
    pub hyde: f32,
    /// Weight for the `HyDE` blend factor — the fraction
    /// of the final score contributed by the `HyDE`
    /// similarity, with the remainder from the direct
    /// per-field cosine similarity. Replaces the
    /// previously hardcoded 0.6 factor.
    ///
    /// Deprecated: LLM `HyDE` is disabled; unused and scheduled for removal.
    #[deprecated(
        note = "LLM HyDE is disabled; this blend factor is unused and scheduled for removal"
    )]
    pub hyde_blend: f32,
}

impl Default for FieldWeights {
    #[expect(
        deprecated,
        reason = "initialize deprecated HyDE weight fields until they are removed"
    )]
    fn default() -> Self {
        Self {
            summary: 1.0,
            description: 0.6,
            capability: 0.8,
            example: 0.4,
            negative: -0.5,
            hyde: 0.7,
            hyde_blend: 0.6,
        }
    }
}

impl From<crate::config::FieldWeightsConfig> for FieldWeights {
    #[expect(
        deprecated,
        reason = "copy deprecated HyDE weight fields until they are removed"
    )]
    fn from(c: crate::config::FieldWeightsConfig) -> Self {
        Self {
            summary: c.summary,
            description: c.description,
            capability: c.capability,
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
    /// Number of top candidates to retrieve from vector search (pre-rerank).
    pub top_k: usize,
    /// Number of final tools to return after reranking.
    pub final_n: usize,
    /// Deprecated: LLM `HyDE` expansion. Currently a no-op; scheduled for removal.
    #[deprecated(note = "LLM HyDE is disabled (no-op); this knob is scheduled for removal")]
    pub use_hyde: bool,
    /// Whether to cosine-rerank candidates (no LLM).
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
    /// Cap how many tools per category may appear (`ToolCategory::config_key`).
    pub per_category_limits: HashMap<String, usize>,
}

impl Default for ToolRagOptions {
    #[expect(
        deprecated,
        reason = "initialize deprecated use_hyde until it is removed"
    )]
    fn default() -> Self {
        Self {
            enabled: true,
            top_k: 12,
            final_n: 6,
            use_hyde: false,
            use_rerank: false,
            rerank_candidates: 24,
            min_similarity: 0.25,
            background_index_on_startup: true,
            forced: vec![
                ToolName::new("utility.question"),
                ToolName::new("utility.todo_add"),
                ToolName::new("utility.get_current_time"),
            ],
            weights: FieldWeights::default(),
            per_category_limits: HashMap::new(),
        }
    }
}

impl TryFrom<crate::config::ToolRagConfig> for ToolRagOptions {
    type Error = crate::ToolRagError;

    fn try_from(c: crate::config::ToolRagConfig) -> Result<Self, Self::Error> {
        Self::from_config(c)
    }
}

impl ToolRagOptions {
    /// Builds options from config. Invalid `forced` names fail construction.
    #[expect(deprecated, reason = "copy deprecated use_hyde until it is removed")]
    pub fn from_config(c: crate::config::ToolRagConfig) -> Result<Self, crate::ToolRagError> {
        let mut forced = Vec::with_capacity(c.forced.len());
        for name in c.forced {
            match ToolName::try_new(name) {
                Ok(tn) => forced.push(tn),
                Err(e) => {
                    return Err(crate::ToolRagError::Config {
                        message: format!("invalid tool name in rag.forced: {e}"),
                    });
                }
            }
        }
        if c.use_hyde {
            tracing::warn!(
                component = "ToolRag",
                "tools.rag.use_hyde is deprecated and ignored (LLM HyDE disabled; scheduled for removal)"
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
            per_category_limits: c.per_category_limits,
        })
    }
}

// ── Pipeline ──────────────────────────────────────────────────────────────

/// The Tool RAG pipeline: embed → weighted field similarity →
/// optional cosine rerank → top-N.
pub struct ToolRag {
    embedder: Arc<dyn EmbeddingProvider>,
    store: Option<Arc<MemoryStore>>,
    opts: ToolRagOptions,
    /// Last-known `ToolSpecs`, used when returning results from [`select`].
    specs: RwLock<HashMap<ToolName, ToolSpec>>,
    /// Last-known RAG profiles (for category limits).
    profiles: RwLock<HashMap<ToolName, ToolRagProfile>>,
    /// Blake3 hash of the last indexed specs+profiles set.
    /// Used to fast-path `ensure_index` when inputs haven't changed.
    last_specs_hash: AtomicU64,
    /// In-memory cache of tool embedding vectors.
    /// Populated by `ensure_index`, used by select. Avoids
    /// deserializing all f32 vecs from `SQLite` every turn.
    /// Wrapped in `Arc` so `select` can clone the handle instead
    /// of copying every cached embedding vector per query.
    #[expect(
        clippy::rc_buffer,
        reason = "Arc<Vec> is intentional: select clones the handle, not the embeddings"
    )]
    cached_field_rows: RwLock<Arc<Vec<CachedFieldRow>>>,
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
            profiles: RwLock::new(HashMap::new()),
            last_specs_hash: AtomicU64::new(0),
            cached_field_rows: RwLock::new(Arc::new(Vec::new())),
        }
    }

    /// Construct from a config-level `ToolRagConfig`.
    ///
    /// Invalid `forced` names fail construction.
    pub fn from_config(
        embedder: Arc<dyn EmbeddingProvider>,
        store: Option<Arc<MemoryStore>>,
        config: crate::config::ToolRagConfig,
    ) -> Result<Self, crate::ToolRagError> {
        let opts = ToolRagOptions::from_config(config)?;
        Ok(Self::new(embedder, store, opts))
    }

    fn forced_only_specs(&self) -> Vec<ToolSpec> {
        let all_specs = self
            .specs
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut result = Vec::new();
        for forced_name in &self.opts.forced {
            if let Some(spec) = all_specs.get(forced_name) {
                result.push(spec.clone());
            }
        }
        result
    }

    /// Returns a reference to the runtime options.
    pub const fn opts(&self) -> &ToolRagOptions {
        &self.opts
    }

    /// Whether the pipeline has a backing memory store.
    pub const fn has_store(&self) -> bool {
        self.store.is_some()
    }

    fn store_specs_and_profiles(&self, specs: &[ToolSpec], profiles: &[ToolRagProfile]) {
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
        {
            let mut map = self
                .profiles
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            map.clear();
            for profile in profiles {
                map.insert(profile.name.clone(), profile.clone());
            }
            // Fill gaps with synthesized profiles from specs.
            for spec in specs {
                map.entry(spec.name.clone())
                    .or_insert_with(|| ToolRagProfile::from_tool_spec(spec));
            }
        }
    }

    // ── Indexing ───────────────────────────────────────────────────────

    /// Ensures every tool is embedded and stored using matching profiles.
    ///
    /// Specs without a profile are indexed from a synthesized
    /// [`ToolRagProfile::from_tool_spec`]. Only re-embeds fields whose
    /// text-derived version hash has changed.
    pub async fn ensure_index(
        &self,
        specs: &[ToolSpec],
        profiles: &[ToolRagProfile],
    ) -> Result<(), EmbeddingError> {
        let specs_hash = compute_index_hash(specs, profiles);
        let prev_hash = self.last_specs_hash.load(Ordering::Acquire);
        {
            let cache = self
                .cached_field_rows
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if prev_hash == specs_hash && !cache.is_empty() {
                self.store_specs_and_profiles(specs, profiles);
                return Ok(());
            }
        }

        let Some(store) = self.store.as_ref().map(Arc::clone) else {
            self.store_specs_and_profiles(specs, profiles);
            return Ok(());
        };

        let profile_by_name: HashMap<&str, &ToolRagProfile> =
            profiles.iter().map(|p| (p.name.as_str(), p)).collect();

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
            let owned;
            let profile = if let Some(p) = profile_by_name.get(spec.name.as_str()) {
                *p
            } else {
                owned = ToolRagProfile::from_tool_spec(spec);
                &owned
            };

            self.index_field(
                &store,
                &cached,
                &model_name,
                profile,
                EmbeddingField::Summary,
                "",
                None,
                Some(&spec.parameters),
            )
            .await?;

            self.index_field(
                &store,
                &cached,
                &model_name,
                profile,
                EmbeddingField::Description,
                "",
                None,
                Some(&spec.parameters),
            )
            .await?;

            self.index_field(
                &store,
                &cached,
                &model_name,
                profile,
                EmbeddingField::Capability,
                "",
                None,
                None,
            )
            .await?;

            self.index_field(
                &store,
                &cached,
                &model_name,
                profile,
                EmbeddingField::Negative,
                "",
                None,
                None,
            )
            .await?;

            for i in 0..profile.examples.len() {
                let field_key = format!("ex_{i}");
                self.index_field(
                    &store,
                    &cached,
                    &model_name,
                    profile,
                    EmbeddingField::Example,
                    &field_key,
                    Some(i),
                    None,
                )
                .await?;
            }
        }

        self.store_specs_and_profiles(specs, profiles);

        match store.list_tool_embedding_fields().await {
            Ok(rows) => {
                let mapped: Vec<CachedFieldRow> = rows
                    .into_iter()
                    .filter_map(|row| {
                        Some(CachedFieldRow {
                            tool_name: row.tool_name,
                            field: EmbeddingField::from_field_name(&row.field)?,
                            embedding: row.embedding,
                        })
                    })
                    .collect();
                let mut cache_write = self
                    .cached_field_rows
                    .write()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                *cache_write = Arc::new(mapped);
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

    async fn index_field(
        &self,
        store: &Arc<MemoryStore>,
        cached: &HashMap<(String, String, String), (String, String)>,
        model_name: &str,
        profile: &ToolRagProfile,
        field: EmbeddingField,
        field_key: &str,
        example_index: Option<usize>,
        parameters: Option<&serde_json::Value>,
    ) -> Result<(), EmbeddingError> {
        let text = profile.embedding_text(field, parameters, example_index);
        if text.is_empty() {
            return Ok(());
        }
        let field_name = field.as_str();
        let key = (
            profile.name.as_str().to_string(),
            field_name.to_string(),
            field_key.to_string(),
        );
        let hash = field_version_hash(field_name, &text);
        if is_cached(cached, &key, &hash, model_name) {
            return Ok(());
        }
        let kind = match field {
            EmbeddingField::Summary => ene_ai::EmbeddingKind::Summary,
            EmbeddingField::Description => ene_ai::EmbeddingKind::Description,
            EmbeddingField::Capability => ene_ai::EmbeddingKind::Capability,
            EmbeddingField::Example => ene_ai::EmbeddingKind::Example,
            EmbeddingField::Negative => ene_ai::EmbeddingKind::Negative,
        };
        let emb = embed(self.embedder.as_ref(), &text, kind).await?;
        persist(
            store,
            profile.name.as_str(),
            field_name,
            field_key,
            &hash,
            model_name,
            &emb,
            &text,
        )
        .await
    }

    // ── Selection ──────────────────────────────────────────────────────

    /// Select the most relevant tools for the given query.
    ///
    /// Pipeline: embed query → per-tool weighted field similarity →
    /// category limits → `top_k` → optional cosine rerank → `final_n` + forced.
    /// On embed failure, returns forced tools only (fail-closed).
    /// LLM `HyDE` is deprecated and ignored (`use_hyde` is a no-op).
    pub async fn select(&self, query: &str) -> Vec<ToolSpec> {
        let query_vec = match embed_query(self.embedder.as_ref(), query).await {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(
                    component = "ToolRag",
                    error = %e,
                    "Query embedding failed; returning forced tools only"
                );
                return self.forced_only_specs();
            }
        };

        self.select_with_embedding(query, &query_vec).await
    }

    /// Select the most relevant tools using a pre-computed query embedding.
    #[expect(
        deprecated,
        reason = "read deprecated use_hyde for the no-op deprecation warning"
    )]
    pub async fn select_with_embedding(
        &self,
        query: &str,
        query_embedding: &[f32],
    ) -> Vec<ToolSpec> {
        if is_zero_norm(query_embedding) {
            return self.forced_only_specs();
        }

        let t_start = std::time::Instant::now();
        let Some(store) = &self.store else {
            tracing::warn!(
                component = "ToolRag",
                "No memory store; returning forced tools only"
            );
            return self.forced_only_specs();
        };

        if self.opts.use_hyde {
            tracing::warn!(
                component = "ToolRag",
                "use_hyde is deprecated and ignored (LLM HyDE disabled; scheduled for removal)"
            );
        }

        // Clone the `Arc` handle (not the embeddings) so the read
        // lock is released before any `.await` below.
        let cached_rows: Option<Arc<Vec<CachedFieldRow>>> = {
            let cache = self
                .cached_field_rows
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if cache.is_empty() {
                None
            } else {
                Some(Arc::clone(&cache))
            }
        };

        let field_rows: Arc<Vec<CachedFieldRow>> = match cached_rows {
            Some(rows) => rows,
            None => match store.list_tool_embedding_fields().await {
                Ok(rows) => {
                    let mapped: Vec<CachedFieldRow> = rows
                        .into_iter()
                        .filter_map(|row| {
                            Some(CachedFieldRow {
                                tool_name: row.tool_name,
                                field: EmbeddingField::from_field_name(&row.field)?,
                                embedding: row.embedding,
                            })
                        })
                        .collect();
                    let shared = Arc::new(mapped);
                    let mut cache_write = self
                        .cached_field_rows
                        .write()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    *cache_write = Arc::clone(&shared);
                    shared
                }
                Err(e) => {
                    tracing::warn!(component = "ToolRag", error = %e, "Could not load embeddings");
                    Arc::new(Vec::new())
                }
            },
        };
        let t_load = t_start.elapsed();

        let scored = score_tools(
            &field_rows,
            query_embedding,
            &self.opts.weights,
            self.opts.min_similarity,
            &self.opts.per_category_limits,
            &self
                .profiles
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        );

        // Cap to top_k before rerank.
        let mut scored = scored;
        if scored.len() > self.opts.top_k {
            scored.truncate(self.opts.top_k);
        }

        // Clone only the specs we actually return, instead of cloning
        // the entire specs map up front. The guard is scoped so it
        // is dropped before the rerank `.await` below.
        let mut candidates: Vec<(ToolSpec, f32)> = {
            let all_specs = self
                .specs
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let take_n = self.opts.rerank_candidates.min(scored.len());
            let mut out: Vec<(ToolSpec, f32)> = Vec::with_capacity(take_n);
            for (name, score) in scored.iter().take(take_n) {
                match ToolName::try_new(name.clone()) {
                    Ok(tn) => {
                        if let Some(spec) = all_specs.get(&tn) {
                            out.push((spec.clone(), *score));
                        }
                    }
                    Err(e) => {
                        tracing::warn!(component = "ToolRag", error = %e, "Skipping invalid tool name in RAG index");
                    }
                }
            }
            out
        };
        let t_score = t_start.elapsed();

        if self.opts.use_rerank && candidates.len() > 1 {
            let rerank_specs: Vec<ToolSpec> = candidates.iter().map(|(s, _)| s.clone()).collect();
            match crate::hybrid::rerank_tool_specs(
                self.embedder.as_ref(),
                None,
                query,
                &rerank_specs,
            )
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
            load = ?t_load,
            score = ?t_score.checked_sub(t_load).unwrap_or_default(),
            rerank = ?t_rerank.checked_sub(t_score).unwrap_or_default(),
            "RAG selection timings"
        );

        {
            let all_specs = self
                .specs
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
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

    /// Spawns a background task that warms the index with the given specs and profiles.
    /// Returns immediately; the indexing runs asynchronously.
    pub fn start_background_indexer(
        self: &Arc<Self>,
        specs: Vec<ToolSpec>,
        profiles: Vec<ToolRagProfile>,
    ) {
        let rag = Arc::clone(self);
        tokio::spawn(async move {
            match rag.ensure_index(&specs, &profiles).await {
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
    field: EmbeddingField,
    embedding: Vec<f32>,
}

/// `true` when the embedding is empty, all-zero, or contains NaN.
fn is_zero_norm(emb: &[f32]) -> bool {
    if emb.is_empty() {
        return true;
    }
    let norm_sq: f32 = emb.iter().map(|&x| x * x).sum();
    norm_sq == 0.0 || norm_sq.is_nan()
}

/// Core scoring + filtering pipeline, extracted for testability.
///
/// Returns `(tool_name, score)` pairs sorted descending by score,
/// filtered by `min_similarity` and per-category limits.
fn score_tools(
    field_rows: &[CachedFieldRow],
    query_embedding: &[f32],
    weights: &FieldWeights,
    min_similarity: f32,
    per_category_limits: &HashMap<String, usize>,
    profiles: &HashMap<ToolName, ToolRagProfile>,
) -> Vec<(String, f32)> {
    let mut per_tool: HashMap<String, f32> = HashMap::new();

    for row in field_rows {
        if is_zero_norm(&row.embedding) {
            continue;
        }
        let sim = cosine_similarity(query_embedding, &row.embedding);

        let weight = match row.field {
            EmbeddingField::Summary => weights.summary,
            EmbeddingField::Description => weights.description,
            EmbeddingField::Capability => weights.capability,
            EmbeddingField::Example => weights.example,
            EmbeddingField::Negative => weights.negative,
        };

        *per_tool.entry(row.tool_name.clone()).or_insert(0.0) += sim * weight;
    }

    let mut scored: Vec<(String, f32)> = per_tool
        .into_iter()
        .filter(|(_, s)| *s >= min_similarity)
        .collect();

    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

    if !per_category_limits.is_empty() {
        let mut category_counts: HashMap<String, usize> = HashMap::new();
        scored.retain(|(name, _)| {
            let Ok(tn) = ToolName::try_new(name.clone()) else {
                return false;
            };
            let Some(profile) = profiles.get(&tn) else {
                return true;
            };
            let key = profile.category.config_key().to_string();
            let Some(&limit) = per_category_limits.get(&key) else {
                return true;
            };
            let count = category_counts.entry(key).or_insert(0);
            if *count >= limit {
                return false;
            }
            *count += 1;
            true
        });
    }

    scored
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

fn compute_index_hash(specs: &[ToolSpec], profiles: &[ToolRagProfile]) -> u64 {
    let mut hasher = blake3::Hasher::new();
    for spec in specs {
        if let Ok(bytes) = serde_json::to_vec(spec) {
            hasher.update(&bytes);
        } else {
            hasher.update(spec.name.as_str().as_bytes());
        }
    }
    hasher.update(b"|profiles|");
    for profile in profiles {
        if let Ok(bytes) = serde_json::to_vec(profile) {
            hasher.update(&bytes);
        } else {
            hasher.update(profile.name.as_str().as_bytes());
        }
    }
    let hash = hasher.finalize();
    let bytes = hash.as_bytes();
    let mut array = [0u8; 8];
    array.copy_from_slice(&bytes[0..8]);
    u64::from_le_bytes(array)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ene_tool_proto::types::KeywordSet;
    use ene_tool_proto::{ToolCategory, ToolExample, ToolVersion};

    fn profile(name: &str, category: ToolCategory) -> ToolRagProfile {
        ToolRagProfile {
            name: ToolName::new(name),
            display_name: name.into(),
            summary: format!("summary for {name}"),
            description: format!("description for {name}"),
            category,
            keywords: KeywordSet::default(),
            examples: vec![ToolExample {
                description: "ex".into(),
                input: serde_json::json!({}),
                output: None,
            }],
            caveats: Vec::new(),
            preconditions: Vec::new(),
            side_effects: ene_tool_proto::SideEffects::ReadOnly,
            related: Vec::new(),
            version: ToolVersion::default(),
        }
    }

    #[test]
    fn index_hash_includes_profiles() {
        let specs = vec![ToolSpec::new(
            ToolName::new("a"),
            "desc",
            serde_json::json!({}),
        )];
        let p1 = vec![profile("a", ToolCategory::Utility)];
        let mut p2 = p1.clone();
        p2[0].keywords.negative.push("nope".into());
        assert_ne!(
            compute_index_hash(&specs, &p1),
            compute_index_hash(&specs, &p2)
        );
    }

    #[test]
    fn from_config_rejects_invalid_forced() {
        let cfg = crate::config::ToolRagConfig {
            top_k: 3,
            final_n: 2,
            use_rerank: true,
            min_similarity: 0.5,
            forced: vec![
                "utility.question".into(),
                "NOT A VALID NAME!!!".into(),
                "utility.todo_add".into(),
            ],
            ..crate::config::ToolRagConfig::default()
        };
        #[expect(clippy::expect_used, reason = "unit test asserts Err")]
        let err = ToolRagOptions::from_config(cfg).expect_err("invalid forced");
        assert!(
            err.to_string().contains("rag.forced"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn from_config_accepts_valid_forced() {
        let cfg = crate::config::ToolRagConfig {
            top_k: 3,
            final_n: 2,
            forced: vec!["utility.question".into(), "utility.todo_add".into()],
            ..crate::config::ToolRagConfig::default()
        };
        #[expect(clippy::expect_used, reason = "unit test asserts Ok")]
        let opts = ToolRagOptions::from_config(cfg).expect("valid");
        assert_eq!(opts.top_k, 3);
        assert_eq!(opts.final_n, 2);
        assert_eq!(opts.forced.len(), 2);
        assert_eq!(opts.forced[0].as_str(), "utility.question");
        assert_eq!(opts.forced[1].as_str(), "utility.todo_add");
    }

    #[test]
    fn defaults_disable_hyde_and_rerank() {
        let cfg = crate::config::ToolRagConfig::default();
        #[expect(deprecated, reason = "assert deprecated default")]
        {
            assert!(!cfg.use_hyde);
        }
        assert!(!cfg.use_rerank);
        let opts = ToolRagOptions::default();
        #[expect(deprecated, reason = "assert deprecated default")]
        {
            assert!(!opts.use_hyde);
        }
        assert!(!opts.use_rerank);
    }

    // ── score_tools / selection pipeline tests ─────────────────────────────

    fn row(tool: &str, field: EmbeddingField, embedding: Vec<f32>) -> CachedFieldRow {
        CachedFieldRow {
            tool_name: tool.into(),
            field,
            embedding,
        }
    }

    #[test]
    fn is_zero_norm_detects_empty_zero_and_nan() {
        assert!(is_zero_norm(&[]));
        assert!(is_zero_norm(&[0.0, 0.0, 0.0]));
        assert!(is_zero_norm(&[f32::NAN, 1.0]));
        assert!(!is_zero_norm(&[1.0, 0.0]));
    }

    #[test]
    fn score_tools_ranks_by_weighted_similarity() {
        // query points along the x-axis.
        let query = vec![1.0, 0.0];
        let rows = vec![
            row("a", EmbeddingField::Summary, vec![1.0, 0.0]),
            row("b", EmbeddingField::Summary, vec![0.0, 1.0]),
        ];
        let weights = FieldWeights::default();
        let scored = score_tools(
            &rows,
            &query,
            &weights,
            0.0,
            &HashMap::new(),
            &HashMap::new(),
        );
        assert_eq!(scored.len(), 2);
        // "a" is perfectly aligned; "b" is orthogonal (score 0).
        assert_eq!(scored[0].0, "a");
        assert!(scored[0].1 > scored[1].1);
    }

    #[test]
    fn score_tools_filters_below_min_similarity() {
        let query = vec![1.0, 0.0];
        let rows = vec![
            row("a", EmbeddingField::Summary, vec![1.0, 0.0]),
            row("b", EmbeddingField::Summary, vec![0.0, 1.0]),
        ];
        let weights = FieldWeights::default();
        // min_similarity = 0.5 filters out the orthogonal "b".
        let scored = score_tools(
            &rows,
            &query,
            &weights,
            0.5,
            &HashMap::new(),
            &HashMap::new(),
        );
        assert_eq!(scored.len(), 1);
        assert_eq!(scored[0].0, "a");
    }

    #[test]
    fn score_tools_skips_zero_norm_rows() {
        let query = vec![1.0, 0.0];
        let rows = vec![
            row("a", EmbeddingField::Summary, vec![1.0, 0.0]),
            row("b", EmbeddingField::Summary, vec![0.0, 0.0]),
        ];
        let weights = FieldWeights::default();
        let scored = score_tools(
            &rows,
            &query,
            &weights,
            0.0,
            &HashMap::new(),
            &HashMap::new(),
        );
        assert_eq!(scored.len(), 1);
        assert_eq!(scored[0].0, "a");
    }

    #[test]
    fn score_tools_applies_per_category_limits() {
        let query = vec![1.0, 0.0];
        // Two tools in the same category; limit to 1.
        let rows = vec![
            row("utility.a", EmbeddingField::Summary, vec![1.0, 0.0]),
            row("utility.b", EmbeddingField::Summary, vec![0.9, 0.1]),
        ];
        let weights = FieldWeights::default();
        let mut profiles = HashMap::new();
        profiles.insert(
            ToolName::new("utility.a"),
            profile("utility.a", ToolCategory::Utility),
        );
        profiles.insert(
            ToolName::new("utility.b"),
            profile("utility.b", ToolCategory::Utility),
        );
        let mut limits = HashMap::new();
        limits.insert("Utility".to_string(), 1);

        let scored = score_tools(&rows, &query, &weights, 0.0, &limits, &profiles);
        assert_eq!(scored.len(), 1);
        assert_eq!(scored[0].0, "utility.a");
    }

    #[test]
    fn score_tools_negative_weight_penalizes() {
        let query = vec![1.0, 0.0];
        // "a" has a strong negative-field match; its score should be reduced.
        let rows = vec![
            row("a", EmbeddingField::Summary, vec![1.0, 0.0]),
            row("a", EmbeddingField::Negative, vec![1.0, 0.0]),
            row("b", EmbeddingField::Summary, vec![0.8, 0.0]),
        ];
        let weights = FieldWeights::default();
        let scored = score_tools(
            &rows,
            &query,
            &weights,
            -10.0,
            &HashMap::new(),
            &HashMap::new(),
        );
        // "a" score = 1.0*1.0 + 1.0*(-0.5) = 0.5; "b" score = 0.8*1.0 = 0.8.
        assert_eq!(scored[0].0, "b");
    }
}
