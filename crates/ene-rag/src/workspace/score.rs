use crate::scoring::lexical_overlap_score;

/// Hybrid relevance for one chunk in `[0, 1]`.
///
/// Blends the vector cosine similarity with the CJK-aware lexical overlap
/// score, so a chunk that matches the query's wording still surfaces when the
/// embedding model misses it and vice versa. The blend is weighted toward the
/// vector (the stronger generalizer); lexical-only search uses the overlap
/// score directly.
pub fn score_chunk(query_text: &str, content: &str, vector_similarity: Option<f32>) -> f32 {
    let lexical = lexical_overlap_score(query_text, "", content);
    match vector_similarity {
        Some(vector) => (0.6 * vector + 0.4 * lexical).clamp(0.0, 1.0),
        None => lexical,
    }
}

#[cfg(test)]
mod tests {
    use super::score_chunk;

    #[test]
    fn blends_vector_and_lexical() {
        let s = score_chunk(
            "installation steps",
            "installation steps for the app",
            Some(0.8),
        );
        assert!(s > 0.6 && s <= 1.0);
    }

    #[test]
    fn lexical_only_uses_overlap() {
        let s = score_chunk("install ene", "install the ene companion", None);
        assert!(s > 0.0);
        let none = score_chunk("zzz qqq", "install the ene companion", None);
        assert!(none.abs() < f32::EPSILON);
    }

    #[test]
    fn vector_without_lexical_still_counts() {
        let s = score_chunk("unrelated", "completely different content", Some(0.9));
        assert!((s - 0.54).abs() < f32::EPSILON);
    }
}
