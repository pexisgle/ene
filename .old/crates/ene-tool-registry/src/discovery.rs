use crate::def::ToolDefinition;

/// Maximum keywords retained per tool in the search index.
pub(crate) const MAX_KEYWORDS: usize = 32;
/// Maximum length of one keyword string.
pub(crate) const MAX_KEYWORD_LEN: usize = 64;
/// Maximum examples retained per tool in the search index.
pub(crate) const MAX_EXAMPLES: usize = 8;
/// Maximum length of one example string.
pub(crate) const MAX_EXAMPLE_LEN: usize = 256;
/// Maximum category string length.
pub(crate) const MAX_CATEGORY_LEN: usize = 64;

/// One ranked tool from [`crate::ToolRegistry::search_tools`].
#[derive(Debug, Clone, PartialEq)]
pub struct ToolHit {
    pub tool: ToolDefinition,
    pub score: u32,
}

#[derive(Debug, Clone)]
pub(crate) struct IndexedTool {
    pub name: String,
    pub description: String,
    pub category: String,
    pub keywords: Vec<String>,
    pub examples: Vec<String>,
}

impl IndexedTool {
    pub(crate) fn from_definition(def: &ToolDefinition) -> Self {
        Self {
            name: def.name.clone(),
            description: def.description.clone(),
            category: truncate_field(&def.category, MAX_CATEGORY_LEN),
            keywords: truncate_list(&def.keywords, MAX_KEYWORDS, MAX_KEYWORD_LEN),
            examples: truncate_list(&def.examples, MAX_EXAMPLES, MAX_EXAMPLE_LEN),
        }
    }
}

fn truncate_field(raw: &str, max_len: usize) -> String {
    if raw.len() <= max_len {
        return raw.to_owned();
    }
    let mut end = max_len;
    while end > 0 && !raw.is_char_boundary(end) {
        end -= 1;
    }
    raw[..end].to_owned()
}

fn truncate_list(items: &[String], max_items: usize, max_len: usize) -> Vec<String> {
    items
        .iter()
        .take(max_items)
        .map(|item| truncate_field(item, max_len))
        .collect()
}

/// Lexical score for one query against indexed discovery fields.
#[must_use]
pub(crate) fn lexical_score(query: &str, entry: &IndexedTool) -> u32 {
    let query = query.trim();
    if query.is_empty() {
        return 0;
    }
    let query_lower = query.to_ascii_lowercase();
    let tokens = tokenize(&query_lower);
    let name = entry.name.to_ascii_lowercase();
    let description = entry.description.to_ascii_lowercase();
    let category = entry.category.to_ascii_lowercase();

    let mut score = 0_u32;
    if name == query_lower {
        score = score.saturating_add(200);
    } else if name.contains(&query_lower) {
        score = score.saturating_add(100);
    }
    if description.contains(&query_lower) {
        score = score.saturating_add(40);
    }
    if category.contains(&query_lower) {
        score = score.saturating_add(30);
    }

    for token in &tokens {
        if token.is_empty() {
            continue;
        }
        if name.contains(token) {
            score = score.saturating_add(20);
        }
        if description.contains(token) {
            score = score.saturating_add(10);
        }
        if category.contains(token) {
            score = score.saturating_add(12);
        }
        for keyword in &entry.keywords {
            if keyword.to_ascii_lowercase().contains(token) {
                score = score.saturating_add(8);
            }
        }
        for example in &entry.examples {
            if example.to_ascii_lowercase().contains(token) {
                score = score.saturating_add(4);
            }
        }
    }
    score
}

fn tokenize(query: &str) -> Vec<String> {
    query
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .filter(|token| !token.is_empty())
        .map(str::to_owned)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{IndexedTool, lexical_score, truncate_list};

    #[test]
    fn truncate_list_caps_count_and_length() {
        let huge = vec!["x".repeat(128); 64];
        let out = truncate_list(&huge, 4, 16);
        assert_eq!(out.len(), 4);
        assert!(out.iter().all(|item| item.len() <= 16));
    }

    #[test]
    fn lexical_score_prefers_name_match() {
        let entry = IndexedTool {
            name: "fs.read".to_owned(),
            description: "read file".to_owned(),
            category: "filesystem".to_owned(),
            keywords: vec!["open".to_owned()],
            examples: Vec::new(),
        };
        let read = lexical_score("read", &entry);
        let open = lexical_score("open", &entry);
        assert!(read > open);
    }
}
