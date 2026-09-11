//! Bounded lexical term overlap for selecting what one use can draw on.
//!
//! This is a derived selection heuristic, never a Memory's stored meaning:
//! semantic importance stays on the Memory, while overlap exists only for the
//! duration of one formation candidate selection or one recall. No embedding,
//! index, or per-query score is persisted.

/// Splits text into lowercase terms.
///
/// Latin words are matched whole; runs containing non-ASCII characters (for
/// example Japanese) also contribute character bigrams, because they rarely
/// appear as whitespace-delimited words.
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

/// Counts how many of `terms` occur in `content`.
pub(crate) fn overlap(terms: &[String], content: &str) -> usize {
    let content = content.to_lowercase();
    terms
        .iter()
        .filter(|term| content.contains(term.as_str()))
        .count()
}
