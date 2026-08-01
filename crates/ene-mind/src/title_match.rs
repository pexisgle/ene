//! Shared title matching for subject identity.
//!
//! Two independent places have to answer the same question — "do these two
//! titles name the same thing?": the memory arbiter deciding whether a
//! candidate contradicts an existing memory, and the commitment ledger deciding
//! whether a candidate restates an active commitment. Both compare by
//! title-embedding cosine similarity when an [`EmbeddingProvider`] is
//! configured, and both fall back to normalized exact equality otherwise.
//!
//! Keeping one implementation matters beyond deduplication: the two copies had
//! already drifted, and only one of them rejected all-zero embedding vectors.
//! On the copy that did not, a provider returning zero vectors made every
//! similarity comparison fail while still reporting success, so the ledger
//! silently stopped recognizing commitments it had already stored.

use std::collections::HashMap;
use std::sync::Arc;

use ene_ai::{EmbeddingKind, EmbeddingProvider, cosine_similarity};
use tracing::warn;
use unicode_normalization::UnicodeNormalization;

/// Normalize a title for exact-equality matching.
///
/// NFKC folds width and compatibility variants together (`ＡＢ` vs `AB`), then
/// case, whitespace, and punctuation are dropped so that titles differing only
/// in surface formatting compare equal. This is deliberately aggressive: the
/// question being answered is "same subject?", not "same string".
#[must_use]
pub(crate) fn normalize_title(s: &str) -> String {
    s.nfkc()
        .collect::<String>()
        .to_lowercase()
        .chars()
        .filter(|c| !is_separator_punctuation(*c) && !c.is_whitespace())
        .collect()
}

/// Punctuation dropped by [`normalize_title`]: ASCII punctuation plus the
/// CJK marks that carry no subject information.
pub(crate) const fn is_separator_punctuation(c: char) -> bool {
    c.is_ascii_punctuation()
        || matches!(
            c,
            '。' | '、'
                | '・'
                | '！'
                | '？'
                | '「'
                | '」'
                | '『'
                | '』'
                | '（'
                | '）'
                | '…'
                | '〜'
                | 'ー'
                | '—'
                | '–'
                | '．'
                | '，'
                | '：'
                | '；'
        )
}

/// How [`TitleMatcher`] compares two titles for a single scan.
///
/// Pinned at scan start (after prefetch) so a mid-scan embedding failure cannot
/// mix cosine-similarity matches with exact-title fallback inside one pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TitleMatchMode {
    /// Compare by title-embedding cosine similarity against the threshold.
    Embedding,
    /// Compare by normalized exact title equality.
    Exact,
}

/// Decides whether two titles refer to the same subject.
///
/// With an [`EmbeddingProvider`] configured, titles are compared by cosine
/// similarity and match once they reach the configured threshold — this is what
/// lets "資料をまとめる" and "資料作成" collapse into one subject without a
/// hardcoded synonym list. Without an embedder (or after an embedding failure)
/// the matcher degrades to [`normalize_title`] equality, so matching never
/// silently stops running.
///
/// Embeddings are cached by title as [`Arc<[f32]>`] for the matcher's lifetime,
/// so a scan that compares one candidate against many existing titles embeds
/// each distinct title once and pairwise similarity only bumps refcounts.
pub(crate) struct TitleMatcher<'a> {
    embedder: Option<&'a dyn EmbeddingProvider>,
    threshold: f32,
    cache: HashMap<String, Arc<[f32]>>,
    disabled: bool,
    degraded: bool,
    /// `component` field on degrade warnings, so a log line names the caller
    /// (contradiction check vs commitment ledger) rather than this module.
    component: &'static str,
}

impl<'a> TitleMatcher<'a> {
    /// Create a matcher. Embeddings are computed lazily and cached.
    pub(crate) fn new(
        embedder: Option<&'a dyn EmbeddingProvider>,
        threshold: f32,
        component: &'static str,
    ) -> Self {
        Self {
            embedder,
            threshold,
            cache: HashMap::new(),
            disabled: false,
            degraded: false,
            component,
        }
    }

    /// The comparison mode to pin for a scan, based on current matcher health.
    pub(crate) fn match_mode(&self) -> TitleMatchMode {
        if self.use_embedding() {
            TitleMatchMode::Embedding
        } else {
            TitleMatchMode::Exact
        }
    }

    /// Whether the embedding path failed and the matcher fell back to exact
    /// matching. Callers redo the scan under [`TitleMatchMode::Exact`] rather
    /// than accept a pass that mixed both semantics.
    pub(crate) const fn embedding_degraded(&self) -> bool {
        self.degraded
    }

    fn use_embedding(&self) -> bool {
        self.embedder.is_some() && !self.disabled
    }

    fn note_degraded(&mut self, detail: &str) {
        warn!(
            component = self.component,
            degraded = true,
            detail = %detail,
            "title embedding unavailable; falling back to exact title matching"
        );
        self.disabled = true;
        self.degraded = true;
    }

    /// Embed every distinct title in `titles` with a single provider call.
    ///
    /// Called once per scan with all titles about to be compared, so a scan
    /// issues one `embed_batch` instead of one round trip per title.
    pub(crate) async fn prefetch<'t>(&mut self, titles: impl IntoIterator<Item = &'t str>) {
        if !self.use_embedding() {
            return;
        }
        let missing: Vec<&str> = titles
            .into_iter()
            .filter(|title| !self.cache.contains_key(*title))
            .collect();
        if missing.is_empty() {
            return;
        }
        let Some(embedder) = self.embedder else {
            return;
        };
        let items: Vec<(&str, EmbeddingKind)> = missing
            .iter()
            .map(|t| (*t, EmbeddingKind::Summary))
            .collect();
        match embedder.embed_batch(&items).await {
            Ok(vectors) if vectors.len() == missing.len() => {
                for (title, vector) in missing.iter().zip(vectors) {
                    if is_zero_vector(&vector) {
                        self.note_degraded("zero vector");
                        self.cache.clear();
                        return;
                    }
                    self.cache.insert((*title).to_string(), Arc::from(vector));
                }
            }
            Ok(vectors) => {
                self.note_degraded(&format!(
                    "batch size mismatch (returned {}, requested {})",
                    vectors.len(),
                    missing.len()
                ));
            }
            Err(error) => {
                self.note_degraded(&format!("provider error: {error}"));
            }
        }
    }

    /// Ensure `title` has a cached embedding, embedding it on demand.
    async fn ensure_embedded(&mut self, title: &str) -> bool {
        if !self.use_embedding() {
            return false;
        }
        if self.cache.contains_key(title) {
            return true;
        }
        let Some(embedder) = self.embedder else {
            return false;
        };
        match embedder
            .embed_batch(&[(title, EmbeddingKind::Summary)])
            .await
        {
            Ok(mut vectors) if vectors.len() == 1 => {
                let vector = vectors.swap_remove(0);
                if is_zero_vector(&vector) {
                    self.note_degraded("zero vector");
                    return false;
                }
                self.cache.insert(title.to_string(), Arc::from(vector));
                true
            }
            Ok(vectors) => {
                self.note_degraded(&format!("batch size mismatch (returned {})", vectors.len()));
                false
            }
            Err(error) => {
                self.note_degraded(&format!("provider error: {error}"));
                false
            }
        }
    }

    /// Cosine similarity between two titles' embeddings, embedding on demand.
    async fn similarity(&mut self, left: &str, right: &str) -> Option<f32> {
        if !self.ensure_embedded(left).await || !self.ensure_embedded(right).await {
            return None;
        }
        let left_embedding = Arc::clone(self.cache.get(left)?);
        let right_embedding = Arc::clone(self.cache.get(right)?);
        Some(cosine_similarity(&left_embedding, &right_embedding))
    }

    /// Whether two titles name the same subject under an explicit mode.
    ///
    /// Callers pin the mode for a whole scan via [`Self::match_mode`] so a
    /// mid-scan failure cannot mix semantics; check [`Self::embedding_degraded`]
    /// afterwards and redo the scan under [`TitleMatchMode::Exact`] if set.
    pub(crate) async fn is_match_with(
        &mut self,
        mode: TitleMatchMode,
        candidate_title: &str,
        existing_title: &str,
    ) -> bool {
        match mode {
            TitleMatchMode::Embedding => self
                .similarity(candidate_title, existing_title)
                .await
                .is_some_and(|similarity| similarity >= self.threshold),
            TitleMatchMode::Exact => {
                normalize_title(candidate_title) == normalize_title(existing_title)
            }
        }
    }

    /// Index of the title in `titles` that best matches `candidate_title`.
    ///
    /// Under [`TitleMatchMode::Exact`] this is the first normalized equality;
    /// under [`TitleMatchMode::Embedding`] it is the highest similarity at or
    /// above the threshold. Returning the single best index (rather than every
    /// title over the threshold) is what keeps a fuzzy match from fanning out
    /// across several near-synonymous subjects.
    pub(crate) async fn best_match_with(
        &mut self,
        mode: TitleMatchMode,
        candidate_title: &str,
        titles: &[&str],
    ) -> Option<usize> {
        match mode {
            TitleMatchMode::Embedding => {
                let mut best: Option<(f32, usize)> = None;
                for (idx, title) in titles.iter().enumerate() {
                    let Some(similarity) = self.similarity(title, candidate_title).await else {
                        continue;
                    };
                    if similarity >= self.threshold
                        && best.is_none_or(|(best_similarity, _)| similarity > best_similarity)
                    {
                        best = Some((similarity, idx));
                    }
                }
                best.map(|(_, idx)| idx)
            }
            TitleMatchMode::Exact => {
                let key = normalize_title(candidate_title);
                titles.iter().position(|t| normalize_title(t) == key)
            }
        }
    }
}

/// An all-zero embedding carries no direction, so cosine similarity against it
/// is meaningless (`0/0`). Providers emit these on internal failure while still
/// reporting success, so they are treated as a failed embedding.
fn is_zero_vector(vector: &[f32]) -> bool {
    vector.iter().all(|v| *v == 0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct ZeroVectorEmbedder;

    #[async_trait::async_trait]
    impl EmbeddingProvider for ZeroVectorEmbedder {
        async fn embed_batch(
            &self,
            items: &[(&str, EmbeddingKind)],
        ) -> Result<Vec<Vec<f32>>, ene_ai::EmbeddingError> {
            Ok(items.iter().map(|_| vec![0.0, 0.0, 0.0]).collect())
        }

        fn dimensions(&self) -> usize {
            3
        }

        fn model_name(&self) -> &'static str {
            "zero-test"
        }
    }

    /// Distinct unit vectors per keyword; unrelated titles are orthogonal.
    struct KeywordEmbedder;

    #[async_trait::async_trait]
    impl EmbeddingProvider for KeywordEmbedder {
        async fn embed_batch(
            &self,
            items: &[(&str, EmbeddingKind)],
        ) -> Result<Vec<Vec<f32>>, ene_ai::EmbeddingError> {
            Ok(items
                .iter()
                .map(|(text, _)| {
                    if text.contains("資料") {
                        vec![1.0, 0.0, 0.0]
                    } else if text.contains("会議") {
                        vec![0.0, 1.0, 0.0]
                    } else {
                        vec![0.0, 0.0, 1.0]
                    }
                })
                .collect())
        }

        fn dimensions(&self) -> usize {
            3
        }

        fn model_name(&self) -> &'static str {
            "keyword-test"
        }
    }

    #[test]
    fn normalize_title_folds_width_case_space_and_punctuation() {
        assert_eq!(
            normalize_title("Ｒｅｐｏｒｔ, v2"),
            normalize_title("report v2")
        );
        assert_eq!(
            normalize_title("資料を まとめる。"),
            normalize_title("資料をまとめる")
        );
        assert_ne!(
            normalize_title("資料をまとめる"),
            normalize_title("会議を設定する")
        );
    }

    #[tokio::test]
    async fn zero_vectors_degrade_instead_of_silently_never_matching() {
        // A provider that returns all-zero vectors reports success, so without
        // an explicit check every similarity comparison would fail and the
        // caller would conclude "no match" for titles that are in fact equal.
        let embedder = ZeroVectorEmbedder;
        let mut matcher = TitleMatcher::new(Some(&embedder), 0.8, "Test");

        assert_eq!(matcher.match_mode(), TitleMatchMode::Embedding);
        let mode = matcher.match_mode();
        assert!(!matcher.is_match_with(mode, "資料", "資料").await);
        assert!(
            matcher.embedding_degraded(),
            "a zero vector must be reported as a degrade so the caller can redo the scan"
        );

        // Redoing the scan under the exact fallback recovers the true answer.
        let mode = matcher.match_mode();
        assert_eq!(mode, TitleMatchMode::Exact);
        assert!(matcher.is_match_with(mode, "資料", "資料").await);
    }

    #[tokio::test]
    async fn zero_vectors_degrade_on_the_prefetch_path_too() {
        let embedder = ZeroVectorEmbedder;
        let mut matcher = TitleMatcher::new(Some(&embedder), 0.8, "Test");

        matcher.prefetch(["資料", "会議"]).await;

        assert!(matcher.embedding_degraded());
        assert_eq!(matcher.match_mode(), TitleMatchMode::Exact);
    }

    #[tokio::test]
    async fn best_match_returns_only_the_single_closest_title() {
        let embedder = KeywordEmbedder;
        let mut matcher = TitleMatcher::new(Some(&embedder), 0.8, "Test");
        let titles = ["会議を設定する", "資料をまとめる", "資料作成"];

        let mode = matcher.match_mode();
        let best = matcher.best_match_with(mode, "資料作成", &titles).await;

        assert!(
            matches!(best, Some(1 | 2)),
            "expected one of the 資料 titles, got {best:?}"
        );
    }

    #[tokio::test]
    async fn without_an_embedder_matching_uses_normalized_equality() {
        let mut matcher = TitleMatcher::new(None, 0.8, "Test");

        assert_eq!(matcher.match_mode(), TitleMatchMode::Exact);
        assert!(
            matcher
                .is_match_with(TitleMatchMode::Exact, "Report v2", "report　v2")
                .await
        );
        assert!(!matcher.embedding_degraded());
    }
}
