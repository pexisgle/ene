//! The [`Scorer`] extension point (#302).
//!
//! A trait-based abstraction over "score one candidate against a context".
//! Each RAG system (memory recall, tool selection, and — later — the document
//! index from #185) keeps its own candidate and context types while sharing the
//! surrounding pipeline (embedding management, caching, decay, rerank, limits).
//!
//! A trait (rather than a string-keyed dynamic registry) is deliberate: the
//! memory side scores [`GatheredCandidate`] into a [`MemoryScoreBreakdown`],
//! while the tool side scores a field embedding into an `f32`. A registry would
//! erase those types; the trait preserves them per system.

use ene_core::{GatheredCandidate, MemoryScoreBreakdown, Query};

use crate::scoring::score_candidate;
#[cfg(feature = "tool")]
use crate::tool::FieldWeights;

/// Score a single candidate against a query context.
///
/// Implementors bind their own [`Candidate`](Scorer::Candidate),
/// [`Context`](Scorer::Context), and [`Score`](Scorer::Score) types so distinct
/// RAG systems share one abstraction without losing type information.
pub trait Scorer {
    /// The unit being scored (e.g. a gathered memory, a tool field embedding).
    type Candidate;
    /// Per-query scoring context (weights, anchors, thresholds).
    type Context<'a>;
    /// The produced score (a breakdown struct, a scalar, ...).
    type Score;

    /// Score `candidate` under `ctx`.
    fn score(&self, ctx: Self::Context<'_>, candidate: &Self::Candidate) -> Self::Score;
}

/// Memory-recall scorer: [`GatheredCandidate`] → [`MemoryScoreBreakdown`].
///
/// Stateless — all policy lives in the [`Query`] context. The first concrete
/// [`Scorer`] implementor.
#[derive(Debug, Clone, Copy, Default)]
pub struct MemoryScorer;

impl Scorer for MemoryScorer {
    type Candidate = GatheredCandidate;
    type Context<'a> = &'a Query<'a>;
    type Score = MemoryScoreBreakdown;

    fn score(&self, ctx: Self::Context<'_>, candidate: &Self::Candidate) -> Self::Score {
        score_candidate(ctx, candidate)
    }
}

/// A single cached tool-field embedding presented for scoring.
#[cfg(feature = "tool")]
#[derive(Debug, Clone)]
pub struct ToolFieldEmbedding {
    /// Namespaced tool name.
    pub tool_name: String,
    /// Which embedding field this vector came from.
    pub field: ene_plugin_proto::tool_types::EmbeddingField,
    /// The embedding vector.
    pub embedding: Vec<f32>,
}

/// Per-query context for tool scoring.
#[cfg(feature = "tool")]
#[derive(Debug, Clone)]
pub struct ToolScoreContext<'a> {
    /// The query embedding vector.
    pub query_embedding: &'a [f32],
    /// Per-field similarity weights.
    pub weights: &'a FieldWeights,
}

/// Tool-selection scorer: a field embedding → weighted cosine contribution.
///
/// Returns the field's cosine similarity multiplied by its weight (negative
/// weights produce a soft penalty). Aggregation across a tool's fields happens
/// in the pipeline, not here. The second concrete [`Scorer`] implementor.
#[cfg(feature = "tool")]
#[derive(Debug, Clone, Copy, Default)]
pub struct ToolScorer;

#[cfg(feature = "tool")]
impl Scorer for ToolScorer {
    type Candidate = ToolFieldEmbedding;
    type Context<'a> = ToolScoreContext<'a>;
    type Score = f32;

    fn score(&self, ctx: Self::Context<'_>, candidate: &Self::Candidate) -> Self::Score {
        if crate::tool::is_zero_norm(&candidate.embedding) {
            return 0.0;
        }
        let sim = ene_ai::cosine_similarity(ctx.query_embedding, &candidate.embedding);
        sim * ctx.weights.for_field(candidate.field)
    }
}

#[cfg(all(test, feature = "tool"))]
mod tests {
    use super::*;
    use crate::tool::FieldWeights;
    use ene_plugin_proto::tool_types::EmbeddingField;

    #[test]
    fn tool_scorer_weights_field_similarity() {
        let scorer = ToolScorer;
        let weights = FieldWeights::default();
        let ctx = ToolScoreContext {
            query_embedding: &[1.0, 0.0],
            weights: &weights,
        };
        let candidate = ToolFieldEmbedding {
            tool_name: "utility.a".into(),
            field: EmbeddingField::Summary,
            embedding: vec![1.0, 0.0],
        };
        let score = scorer.score(ctx, &candidate);
        assert!((score - weights.summary).abs() < 1e-6);
    }

    #[test]
    fn tool_scorer_zero_norm_scores_zero() {
        let scorer = ToolScorer;
        let weights = FieldWeights::default();
        let ctx = ToolScoreContext {
            query_embedding: &[1.0, 0.0],
            weights: &weights,
        };
        let candidate = ToolFieldEmbedding {
            tool_name: "utility.a".into(),
            field: EmbeddingField::Summary,
            embedding: vec![0.0, 0.0],
        };
        assert!(scorer.score(ctx, &candidate).abs() < f32::EPSILON);
    }
}
