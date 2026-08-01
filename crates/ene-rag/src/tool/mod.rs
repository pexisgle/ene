//! Tool RAG pipeline — multi-vector embedding, field-weighted similarity,
//! optional cosine rerank, and per-category limits.
//!
//! The per-tool relevance score is a *normalized* weighted average over the
//! fields a tool actually has (best match per field), so it lives in
//! `[-1, 1]` and does not grow with the number of declared fields or examples.
//! Negative examples act as an exclusion gate rather than a subtracted
//! penalty. This mirrors the relevance-driven structure of the memory hybrid
//! score so the two policies cannot diverge.
//!
//! LLM `HyDE` expansion is disabled and exposes no config knobs; the
//! [`hybrid`] helpers still expose [`hyde_document`](hybrid::hyde_document)
//! for callers that supply their own LLM. Cosine reranking is available but
//! off by default ([`ToolRagOptions::use_rerank`]): the normalized field score
//! is already field-count-independent, so the extra per-query re-embeddings
//! are opt-in.
//!
//! Persistence goes through [`ene_core::EmbeddingStorePort`] rather than the
//! concrete store type, so this crate never depends on `ene-store`.
#![expect(
    clippy::option_if_let_else,
    reason = "nursery style; match/if-let clarity preferred in the tool pipeline"
)]
#![expect(
    clippy::arithmetic_side_effects,
    reason = "tool scoring and timing deltas use intentional arithmetic"
)]
#![expect(
    clippy::indexing_slicing,
    reason = "ranked tool selection indexes into scored candidate lists"
)]

use ene_ai::{EmbeddingError, EmbeddingProvider, cosine_similarity, embed, embed_query};
use ene_core::{EmbeddingStorePort, ToolFailureSignalPort};
use ene_plugin_proto::tool_types::EmbeddingField;
use ene_plugin_proto::{ToolName, ToolRagProfile, ToolSpec};
use parking_lot::RwLock;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

pub mod config;
pub mod error;
pub mod hybrid;

pub use config::{FieldWeightsConfig, ToolRagConfig};
pub use error::ToolRagError;
pub use hybrid::{HybridRerankProvider, hybrid_embed, hyde_document, rerank_tool_specs};

// ── Per-field similarity weights ──────────────────────────────────────────

/// Controls how strongly each embedding field contributes to the per-tool
/// relevance score.
///
/// The score is a weighted average over the fields a tool *actually has*,
/// taking the best match when a field has several embeddings (examples), so it
/// is normalized to `[-1, 1]` and independent of field count. The `negative`
/// field is not averaged in: when a tool's negative-example similarity reaches
/// [`negative_threshold`](Self::negative_threshold) the tool is excluded
/// outright (a gate), so declaring negatives can only ever filter, never
/// penalize a tool relative to one that declares none.
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
    /// Deprecated soft-penalty weight for the negative embedding.
    ///
    /// Negative examples act as a gate ([`negative_threshold`](Self::negative_threshold))
    /// rather than a subtracted score; this field is retained for configuration
    /// compatibility and is not used in scoring.
    pub negative: f32,
    /// Similarity at or above which a tool's negative-example embedding excludes
    /// it from selection. Range `[0, 1]`; `1.0` effectively disables the
    /// gate.
    pub negative_threshold: f32,
}

impl FieldWeights {
    /// The weight for a given embedding field.
    ///
    /// Retained for compatibility with the additive configuration API; the
    /// scoring pipeline uses [`Self::averaging_weight`] so the negative
    /// field is handled as a gate rather than a subtracted component.
    pub const fn for_field(&self, field: EmbeddingField) -> f32 {
        match field {
            EmbeddingField::Summary => self.summary,
            EmbeddingField::Description => self.description,
            EmbeddingField::Capability => self.capability,
            EmbeddingField::Example => self.example,
            EmbeddingField::Negative => self.negative,
        }
    }

    /// The averaging weight for a positive embedding field.
    ///
    /// Returns `None` for [`EmbeddingField::Negative`], which is handled as an
    /// exclusion gate rather than an averaged component.
    pub const fn averaging_weight(&self, field: EmbeddingField) -> Option<f32> {
        match field {
            EmbeddingField::Summary => Some(self.summary),
            EmbeddingField::Description => Some(self.description),
            EmbeddingField::Capability => Some(self.capability),
            EmbeddingField::Example => Some(self.example),
            EmbeddingField::Negative => None,
        }
    }
}

impl Default for FieldWeights {
    fn default() -> Self {
        Self {
            summary: 1.0,
            description: 0.6,
            capability: 0.8,
            example: 0.4,
            negative: -0.5,
            negative_threshold: 0.70,
        }
    }
}

impl From<crate::tool::config::FieldWeightsConfig> for FieldWeights {
    fn from(c: crate::tool::config::FieldWeightsConfig) -> Self {
        Self {
            summary: c.summary,
            description: c.description,
            capability: c.capability,
            example: c.example,
            negative: c.negative,
            negative_threshold: c.negative_threshold,
        }
    }
}

// ── Runtime options (derived from config) ─────────────────────────────────

/// Runtime options for the `ToolRag` pipeline, resolved from
/// [`crate::tool::config::ToolRagConfig`].
#[derive(Debug, Clone)]
pub struct ToolRagOptions {
    /// Whether the RAG pipeline is enabled.
    pub enabled: bool,
    /// Number of top candidates to retrieve from vector search (pre-rerank).
    pub top_k: usize,
    /// Number of final tools to return after reranking.
    pub final_n: usize,
    /// Whether to cosine-rerank candidates (no LLM).
    ///
    /// Defaults to `false`: the weighted field-similarity score is
    /// already normalized and field-count-independent, so the extra
    /// `rerank_candidates` description re-embeddings per query are not worth
    /// their cost by default.
    pub use_rerank: bool,
    /// Number of candidates to consider during reranking.
    pub rerank_candidates: usize,
    /// Minimum normalized similarity (`[-1, 1]`) for a tool to be included.
    pub min_similarity: f32,
    /// Whether to index tools in the background on startup.
    pub background_index_on_startup: bool,
    /// Tools that are always included regardless of RAG scoring.
    pub forced: Vec<ToolName>,
    /// Per-field embedding weights used for scoring.
    pub weights: FieldWeights,
    /// Cap how many tools per category may appear (`ToolCategory::config_key`).
    pub per_category_limits: HashMap<String, usize>,
    /// Whether to down-weight tools with a recent failure memory.
    pub use_failure_feedback: bool,
    /// Score multiplier applied to a recently-failed tool.
    pub failure_penalty: f32,
}

impl Default for ToolRagOptions {
    fn default() -> Self {
        Self {
            enabled: true,
            top_k: 12,
            final_n: 6,
            use_rerank: false,
            rerank_candidates: 24,
            min_similarity: 0.20,
            background_index_on_startup: true,
            forced: vec![
                ToolName::new("utility.question"),
                ToolName::new("utility.todo_add"),
                ToolName::new("utility.get_current_time"),
            ],
            weights: FieldWeights::default(),
            per_category_limits: HashMap::new(),
            use_failure_feedback: true,
            failure_penalty: 0.5,
        }
    }
}

impl TryFrom<crate::tool::config::ToolRagConfig> for ToolRagOptions {
    type Error = crate::tool::ToolRagError;

    fn try_from(c: crate::tool::config::ToolRagConfig) -> Result<Self, Self::Error> {
        Self::from_config(c)
    }
}

impl ToolRagOptions {
    /// Builds options from config. Invalid `forced` names fail construction.
    pub fn from_config(
        c: crate::tool::config::ToolRagConfig,
    ) -> Result<Self, crate::tool::ToolRagError> {
        let mut forced = Vec::with_capacity(c.forced.len());
        for name in c.forced {
            match ToolName::try_new(name) {
                Ok(tn) => forced.push(tn),
                Err(e) => {
                    return Err(crate::tool::ToolRagError::Config {
                        message: format!("invalid tool name in rag.forced: {e}"),
                    });
                }
            }
        }
        Ok(Self {
            enabled: c.enabled,
            top_k: c.top_k,
            final_n: c.final_n,
            use_rerank: c.use_rerank,
            rerank_candidates: c.rerank_candidates,
            min_similarity: c.min_similarity,
            background_index_on_startup: c.background_index_on_startup,
            forced,
            weights: FieldWeights::from(c.weights),
            per_category_limits: c.per_category_limits,
            use_failure_feedback: c.use_failure_feedback,
            failure_penalty: c.failure_penalty,
        })
    }
}

// ── Pipeline ──────────────────────────────────────────────────────────────

/// The Tool RAG pipeline: embed → weighted field similarity →
/// optional cosine rerank → top-N.
pub struct ToolRag {
    embedder: Arc<dyn EmbeddingProvider>,
    store: Option<Arc<dyn EmbeddingStorePort>>,
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
    /// Optional source of recent tool-failure signals. When present and
    /// [`ToolRagOptions::use_failure_feedback`] is enabled, recently-failed
    /// tools are down-weighted during selection. Read through a port so this
    /// crate never depends on `ene-store`/`ene-mind`.
    failure_signals: Option<Arc<dyn ToolFailureSignalPort>>,
}

impl ToolRag {
    /// Creates a new `ToolRag` instance with the given embedder, optional
    /// embedding store, and options.
    pub fn new(
        embedder: Arc<dyn EmbeddingProvider>,
        store: Option<Arc<dyn EmbeddingStorePort>>,
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
            failure_signals: None,
        }
    }

    /// Attach a source of recent tool-failure signals.
    ///
    /// Consumes and returns `self` so it can be chained at construction. When
    /// set (and [`ToolRagOptions::use_failure_feedback`] is enabled), selection
    /// down-weights tools that recently failed for the queried character.
    #[must_use]
    pub fn with_failure_signals(mut self, source: Arc<dyn ToolFailureSignalPort>) -> Self {
        self.failure_signals = Some(source);
        self
    }

    /// Construct from a config-level `ToolRagConfig`.
    ///
    /// Invalid `forced` names fail construction.
    pub fn from_config(
        embedder: Arc<dyn EmbeddingProvider>,
        store: Option<Arc<dyn EmbeddingStorePort>>,
        config: crate::tool::config::ToolRagConfig,
    ) -> Result<Self, crate::tool::ToolRagError> {
        let opts = ToolRagOptions::from_config(config)?;
        Ok(Self::new(embedder, store, opts))
    }

    fn forced_only_specs(&self) -> Vec<ToolSpec> {
        let all_specs = self.specs.read();
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

    /// Whether the pipeline has a backing embedding store.
    pub const fn has_store(&self) -> bool {
        self.store.is_some()
    }

    /// Tools that recently failed for `character_id`, when feedback is on.
    ///
    /// Returns an empty set when failure feedback is disabled, no signal source
    /// is wired, or the lookup fails — selection then proceeds without any
    /// penalty rather than failing the whole query.
    async fn recent_failures(&self, character_id: &str) -> HashSet<String> {
        if !self.opts.use_failure_feedback {
            return HashSet::new();
        }
        let Some(source) = self.failure_signals.as_ref() else {
            return HashSet::new();
        };
        match source.recent_tool_failures(character_id).await {
            Ok(tools) => tools.into_iter().collect(),
            Err(e) => {
                tracing::warn!(
                    component = "ToolRag",
                    error = %e,
                    "Failed to read recent tool failures; continuing without feedback"
                );
                HashSet::new()
            }
        }
    }

    fn store_specs_and_profiles(&self, specs: &[ToolSpec], profiles: &[ToolRagProfile]) {
        {
            let mut map = self.specs.write();
            map.clear();
            for spec in specs {
                map.insert(spec.name.clone(), spec.clone());
            }
        }
        {
            let mut map = self.profiles.write();
            map.clear();
            for profile in profiles {
                map.insert(profile.name.clone(), profile.clone());
            }
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
            let cache = self.cached_field_rows.read();
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
                store.as_ref(),
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
                store.as_ref(),
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
                store.as_ref(),
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
                store.as_ref(),
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
                    store.as_ref(),
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
                let mut cache_write = self.cached_field_rows.write();
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
        store: &dyn EmbeddingStorePort,
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
    /// recent-failure penalty → category limits → `top_k` → optional cosine
    /// rerank → `final_n` + forced. On embed failure, returns forced tools only
    /// (fail-closed).
    ///
    /// `character_id` scopes the recent-failure feedback: tools that
    /// recently failed for this character are down-weighted.
    pub async fn select(&self, query: &str, character_id: &str) -> Vec<ToolSpec> {
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

        self.select_with_embedding(query, &query_vec, character_id)
            .await
    }

    /// Select the most relevant tools using a pre-computed query embedding.
    ///
    /// `character_id` scopes the recent-failure feedback.
    pub async fn select_with_embedding(
        &self,
        query: &str,
        query_embedding: &[f32],
        character_id: &str,
    ) -> Vec<ToolSpec> {
        if is_zero_norm(query_embedding) {
            return self.forced_only_specs();
        }

        let t_start = std::time::Instant::now();
        let Some(store) = self.store.as_ref().map(Arc::clone) else {
            tracing::warn!(
                component = "ToolRag",
                "No embedding store; returning forced tools only"
            );
            return self.forced_only_specs();
        };

        // Clone the `Arc` handle (not the embeddings) so the read
        // lock is released before any `.await` below.
        let cached_rows: Option<Arc<Vec<CachedFieldRow>>> = {
            let cache = self.cached_field_rows.read();
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
                    let mut cache_write = self.cached_field_rows.write();
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

        let failed_tools: HashSet<String> = self.recent_failures(character_id).await;

        let mut scored = score_tools(
            &field_rows,
            query_embedding,
            &self.opts.weights,
            self.opts.min_similarity,
            &self.opts.per_category_limits,
            &self.profiles.read(),
        );

        apply_failure_penalty(
            &mut scored,
            &failed_tools,
            self.opts.failure_penalty,
            self.opts.min_similarity,
        );

        if scored.len() > self.opts.top_k {
            scored.truncate(self.opts.top_k);
        }

        // Clone only the specs we actually return, instead of cloning
        // the entire specs map up front. The guard is scoped so it
        // is dropped before the rerank `.await` below.
        let mut candidates: Vec<(ToolSpec, f32)> = {
            let all_specs = self.specs.read();
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
            match crate::tool::hybrid::rerank_tool_specs(
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
                    // The reranker overwrites the pre-rerank scores, so the
                    // failure penalty applied earlier must be re-applied to
                    // the reranked scores — otherwise a recently-failed tool
                    // could rank first again despite the penalty.
                    apply_rerank_penalty(&mut candidates, &failed_tools, self.opts.failure_penalty);
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
            let all_specs = self.specs.read();
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
pub(crate) fn is_zero_norm(emb: &[f32]) -> bool {
    if emb.is_empty() {
        return true;
    }
    let norm_sq: f32 = emb.iter().map(|&x| x * x).sum();
    norm_sq == 0.0 || norm_sq.is_nan()
}

/// Core scoring + filtering pipeline.
///
/// Returns `(tool_name, score)` pairs sorted descending by score,
/// filtered by `min_similarity` and per-category limits.
///
/// Scoring is normalized and field-count-independent:
/// - For each tool, the best (max) cosine similarity is taken per field, so a
///   tool with many examples is not rewarded beyond its best one.
/// - The positive fields a tool *actually has* are combined as a weighted
///   average `Σ(sim_f × w_f) / Σ(w_f)`, so the score stays in `[-1, 1]`
///   regardless of how many fields the tool declares and `min_similarity` is a
///   genuine similarity threshold.
/// - The `negative` field is an exclusion gate: a tool whose best
///   negative-example similarity reaches `weights.negative_threshold` is
///   dropped, so declaring negatives can only filter, never penalize.
fn score_tools(
    field_rows: &[CachedFieldRow],
    query_embedding: &[f32],
    weights: &FieldWeights,
    min_similarity: f32,
    per_category_limits: &HashMap<String, usize>,
    profiles: &HashMap<ToolName, ToolRagProfile>,
) -> Vec<(String, f32)> {
    let mut best_by_field: HashMap<(String, EmbeddingField), f32> = HashMap::new();
    for row in field_rows {
        if is_zero_norm(&row.embedding) {
            continue;
        }
        let sim = cosine_similarity(query_embedding, &row.embedding);
        let key = (row.tool_name.clone(), row.field);
        let entry = best_by_field.entry(key).or_insert(f32::NEG_INFINITY);
        if sim > *entry {
            *entry = sim;
        }
    }

    let mut weighted_sum: HashMap<String, f32> = HashMap::new();
    let mut weight_total: HashMap<String, f32> = HashMap::new();
    let mut negative_sim: HashMap<String, f32> = HashMap::new();
    for ((tool, field), sim) in best_by_field {
        match weights.averaging_weight(field) {
            Some(weight) if weight > 0.0 => {
                *weighted_sum.entry(tool.clone()).or_insert(0.0) += sim * weight;
                *weight_total.entry(tool).or_insert(0.0) += weight;
            }
            Some(_) => {}
            None => {
                let entry = negative_sim.entry(tool).or_insert(f32::NEG_INFINITY);
                if sim > *entry {
                    *entry = sim;
                }
            }
        }
    }

    let mut scored: Vec<(String, f32)> = weighted_sum
        .into_iter()
        .filter_map(|(tool, sum)| {
            let total_weight = weight_total.get(&tool).copied().unwrap_or(0.0);
            if total_weight <= 0.0 {
                return None;
            }
            if negative_sim
                .get(&tool)
                .is_some_and(|&neg| neg >= weights.negative_threshold)
            {
                return None;
            }
            Some((tool, sum / total_weight))
        })
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

/// Down-weight tools that recently failed for the queried character.
///
/// Multiplies the score of each recently-failed tool by `penalty` (clamped to
/// `[0, 1]`), then drops any tool that falls below `min_similarity`. Tools that
/// stay above the threshold are kept but ranked lower, so a tool that keeps
/// failing is less likely to be re-selected without being hard-excluded.
fn apply_failure_penalty(
    scored: &mut Vec<(String, f32)>,
    failed_tools: &HashSet<String>,
    penalty: f32,
    min_similarity: f32,
) {
    if failed_tools.is_empty() {
        return;
    }
    let penalty = penalty.clamp(0.0, 1.0);
    for (tool, score) in scored.iter_mut() {
        if failed_tools.contains(tool) {
            *score *= penalty;
        }
    }
    scored.retain(|(_, score)| *score >= min_similarity);
    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
}

/// Re-apply the failure penalty to candidates after a rerank pass.
///
/// The reranker overwrites the pre-rerank scores that
/// [`apply_failure_penalty`] already adjusted, so without this the failure
/// signal would be silently discarded from the final ranking. Multiplies the
/// reranked score of each recently-failed tool by `penalty` (clamped to
/// `[0, 1]`); unlike the pre-rerank pass it never drops candidates, keeping
/// the rerank pool composition intact.
fn apply_rerank_penalty(
    candidates: &mut [(ToolSpec, f32)],
    failed_tools: &HashSet<String>,
    penalty: f32,
) {
    if failed_tools.is_empty() || penalty <= 0.0 {
        return;
    }
    let penalty = penalty.clamp(0.0, 1.0);
    for (spec, score) in candidates.iter_mut() {
        if failed_tools.contains(spec.name.as_str()) {
            *score *= penalty;
        }
    }
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
    store: &dyn EmbeddingStorePort,
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
    use ene_plugin_proto::tool_types::KeywordSet;
    use ene_plugin_proto::{ToolCategory, ToolExample, ToolVersion};

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
            side_effects: ene_plugin_proto::SideEffects::ReadOnly,
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
        let cfg = crate::tool::config::ToolRagConfig {
            top_k: 3,
            final_n: 2,
            use_rerank: true,
            min_similarity: 0.5,
            forced: vec![
                "utility.question".into(),
                "NOT A VALID NAME!!!".into(),
                "utility.todo_add".into(),
            ],
            ..crate::tool::config::ToolRagConfig::default()
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
        let cfg = crate::tool::config::ToolRagConfig {
            top_k: 3,
            final_n: 2,
            forced: vec!["utility.question".into(), "utility.todo_add".into()],
            ..crate::tool::config::ToolRagConfig::default()
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
    fn defaults_disable_rerank() {
        let cfg = crate::tool::config::ToolRagConfig::default();
        assert!(!cfg.use_rerank);
        let opts = ToolRagOptions::default();
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
    fn score_tools_negative_field_gates_tool_out() {
        let query = vec![1.0, 0.0];
        // "a" matches the query on its summary but also matches its own
        // negative example strongly — it should be excluded, not penalized.
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
        assert_eq!(scored.len(), 1);
        assert_eq!(scored[0].0, "b");
    }

    #[test]
    fn score_tools_weak_negative_match_does_not_exclude() {
        let query = vec![1.0, 0.0];
        // "a" has a negative example, but it is only weakly similar to the
        // query (below the gate threshold), so "a" stays in the result.
        let rows = vec![
            row("a", EmbeddingField::Summary, vec![1.0, 0.0]),
            row("a", EmbeddingField::Negative, vec![0.0, 1.0]),
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
    fn score_tools_is_independent_of_field_count() {
        let query = vec![1.0, 0.0];
        // Cosine similarity is direction-based, so use a vector at a real
        // angle to the query (sim ~0.707), not merely a shorter one.
        let mediocre = vec![0.5, 0.5];
        let rows = vec![
            // "many": summary + description + capability + 3 examples, all
            // only moderately aligned with the query.
            row("many", EmbeddingField::Summary, mediocre.clone()),
            row("many", EmbeddingField::Description, mediocre.clone()),
            row("many", EmbeddingField::Capability, mediocre.clone()),
            row("many", EmbeddingField::Example, mediocre.clone()),
            row("many", EmbeddingField::Example, mediocre.clone()),
            row("many", EmbeddingField::Example, mediocre),
            // "focused": summary only, perfectly aligned.
            row("focused", EmbeddingField::Summary, vec![1.0, 0.0]),
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
        assert_eq!(scored[0].0, "focused");
        assert!(scored[0].1 <= 1.0);
        assert!(scored[1].1 <= 1.0);
    }

    #[test]
    fn score_tools_multiple_examples_do_not_inflate_score() {
        let query = vec![1.0, 0.0];
        let one_example = vec![row("one", EmbeddingField::Example, vec![0.4, 0.0])];
        let many_examples = vec![
            row("many", EmbeddingField::Example, vec![0.4, 0.0]),
            row("many", EmbeddingField::Example, vec![0.4, 0.0]),
            row("many", EmbeddingField::Example, vec![0.4, 0.0]),
            row("many", EmbeddingField::Example, vec![0.4, 0.0]),
            row("many", EmbeddingField::Example, vec![0.4, 0.0]),
        ];
        let weights = FieldWeights::default();
        let one = score_tools(
            &one_example,
            &query,
            &weights,
            -10.0,
            &HashMap::new(),
            &HashMap::new(),
        );
        let many = score_tools(
            &many_examples,
            &query,
            &weights,
            -10.0,
            &HashMap::new(),
            &HashMap::new(),
        );
        assert_eq!(one.len(), 1);
        assert_eq!(many.len(), 1);
        assert!(
            (one[0].1 - many[0].1).abs() < 1e-6,
            "example count must not change the score: one={} many={}",
            one[0].1,
            many[0].1
        );
    }

    // ── failure-feedback tests ─────────────────────────────────────────────

    #[test]
    fn apply_failure_penalty_downweights_failed_tool() {
        let mut scored = vec![("a".to_string(), 0.8), ("b".to_string(), 0.6)];
        let failed = HashSet::from(["a".to_string()]);
        apply_failure_penalty(&mut scored, &failed, 0.5, 0.0);
        // "a" is penalized to 0.4, dropping below "b" (0.6).
        assert_eq!(scored[0].0, "b");
        let a = scored.iter().find(|(t, _)| t == "a").map(|(_, s)| *s);
        assert_eq!(a, Some(0.4));
    }

    #[test]
    fn apply_failure_penalty_drops_tool_below_min_similarity() {
        let mut scored = vec![("a".to_string(), 0.5), ("b".to_string(), 0.6)];
        let failed = HashSet::from(["a".to_string()]);
        // Penalty pushes "a" to 0.25, below the 0.3 threshold → removed.
        apply_failure_penalty(&mut scored, &failed, 0.5, 0.3);
        assert_eq!(scored.len(), 1);
        assert_eq!(scored[0].0, "b");
    }

    #[test]
    fn apply_failure_penalty_noop_without_failures() {
        let mut scored = vec![("a".to_string(), 0.8), ("b".to_string(), 0.6)];
        apply_failure_penalty(&mut scored, &HashSet::new(), 0.5, 0.0);
        assert_eq!(scored, vec![("a".to_string(), 0.8), ("b".to_string(), 0.6)]);
    }

    /// After a rerank overwrites the pre-rerank scores, the failure penalty
    /// must be re-applied — otherwise a recently-failed tool that the
    /// reranker scores highest would rank first anyway.
    #[test]
    fn rerank_penalty_reapplied_so_failed_tool_cannot_win() {
        use ene_plugin_proto::ToolSpec;

        let spec = |name: &str| {
            ToolSpec::new(
                ToolName::try_new(name.to_string()).unwrap(),
                name.to_string(),
                serde_json::json!({}),
            )
        };
        let mut candidates: Vec<(ToolSpec, f32)> = vec![(spec("a"), 0.95), (spec("b"), 0.90)];
        let failed = HashSet::from(["a".to_string()]);

        apply_rerank_penalty(&mut candidates, &failed, 0.5);
        let a = candidates
            .iter()
            .find(|(s, _)| s.name.as_str() == "a")
            .unwrap()
            .1;
        let b = candidates
            .iter()
            .find(|(s, _)| s.name.as_str() == "b")
            .unwrap()
            .1;
        assert!((a - 0.95 * 0.5).abs() < 1e-6);
        assert!((b - 0.90).abs() < 1e-6);
        assert!(
            b > a,
            "the failed tool must rank below the equal-scored clean tool"
        );
    }

    /// A recently-failed tool is down-weighted end-to-end through the
    /// scoring + penalty composition, so it ranks below an equally-relevant
    /// tool with no recent failure.
    #[test]
    fn recent_failure_lowers_tool_ranking_end_to_end() {
        let query = vec![1.0, 0.0];
        // Both tools are equally relevant on their summary field.
        let rows = vec![
            row("a", EmbeddingField::Summary, vec![1.0, 0.0]),
            row("b", EmbeddingField::Summary, vec![1.0, 0.0]),
        ];
        let weights = FieldWeights::default();
        let mut scored = score_tools(
            &rows,
            &query,
            &weights,
            0.0,
            &HashMap::new(),
            &HashMap::new(),
        );
        apply_failure_penalty(&mut scored, &HashSet::from(["a".to_string()]), 0.5, 0.0);
        assert_eq!(
            scored[0].0, "b",
            "the tool without a recent failure must outrank the failed one"
        );
    }
}
