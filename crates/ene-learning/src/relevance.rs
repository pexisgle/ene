//! Bounded lexical term overlap for selecting what one use can draw on.
//!
//! This is a derived selection heuristic, never a Memory's stored meaning:
//! semantic importance stays on the Memory, while overlap exists only for the
//! duration of one formation candidate selection or one recall. No embedding
//! and no per-query score is persisted; only the derived token set is indexed.

/// Splits text into lowercase terms.
///
/// Latin words are matched whole; runs containing non-ASCII characters (for
/// example Japanese) also contribute character bigrams, because they rarely
/// appear as whitespace-delimited words.
///
/// [`recall_index_terms`] exposes the same split for the durable derived
/// token index, so one tokenizer defines both query terms and indexed terms.
pub(crate) fn terms(text: &str) -> Vec<String> {
    let mut terms = Vec::new();
    for run in text.split(|character: char| !character.is_alphanumeric()) {
        if run.is_empty() {
            continue;
        }
        let lowered = run.to_lowercase();
        let non_ascii = !lowered.is_ascii();
        if non_ascii {
            terms.push(lowered.clone());
            let characters: Vec<char> = lowered.chars().collect();
            for pair in characters.windows(2) {
                terms.push(pair.iter().collect());
            }
        } else if lowered.chars().count() >= 3 {
            terms.push(lowered);
        }
    }
    terms.sort();
    terms.dedup();
    terms
}

pub fn recall_index_terms(content: &str) -> Vec<String> {
    terms(content)
}

/// Counts how many of `query_terms` occur in `content` as whole tokens.
pub(crate) fn overlap(query_terms: &[String], content: &str) -> usize {
    let content_terms = terms(content);
    query_terms
        .iter()
        .filter(|term| {
            content_terms
                .iter()
                .any(|content_term| content_term == *term)
        })
        .count()
}
