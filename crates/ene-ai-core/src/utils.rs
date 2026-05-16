use crate::config::AiSettings;
use crate::create_embedding_provider;
use crate::embedding::EmbeddingProvider;

use crate::memory::store::MemoryStore;
use std::sync::Arc;

/// 文字列を指定長さで切り捨てる（超えた場合は `...` を付与）
pub fn truncate(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        s.to_string()
    } else {
        let end = s
            .char_indices()
            .nth(max_chars)
            .map(|(i, _)| i)
            .unwrap_or(s.len());
        format!("{}...", &s[..end])
    }
}

/// メモリ機能を初期化する
pub fn init_memory(
    settings: &AiSettings,
) -> Result<(Arc<MemoryStore>, Arc<dyn EmbeddingProvider>), String> {
    let db_path = settings.resolve_memory_db_path();

    if let Some(parent) = db_path.parent() {
        if !parent.exists() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create memory DB directory: {}", e))?;
        }
    }

    let embed_base_url = settings
        .resolve_embedding_base_url()
        .map_err(|e| format!("Failed to resolve embedding base URL: {}", e))?;

    let embedder = create_embedding_provider(
        settings.memory.embedding_provider_type,
        &settings.memory.embedding_model,
        &embed_base_url,
        &settings.resolve_api_key(),
        settings.memory.embedding_dimensions.unwrap_or(768),
        Some(&settings.memory.gguf_quantization),
    )
    .map_err(|e| format!("Failed to create embedding provider: {}", e))?;

    let dims = embedder.dimensions();

    let store = MemoryStore::open(&db_path, dims)
        .map_err(|e| format!("Failed to open memory store: {}", e))?;

    Ok((Arc::new(store), Arc::from(embedder)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_truncate_within_limit() {
        assert_eq!(truncate("hello", 10), "hello");
    }

    #[test]
    fn test_truncate_at_limit() {
        assert_eq!(truncate("hello", 5), "hello");
    }

    #[test]
    fn test_truncate_exceeds_limit() {
        assert_eq!(truncate("hello world", 5), "hello...");
    }

    #[test]
    fn test_truncate_unicode() {
        let text = "こんにちは世界";
        // truncate uses char_indices().nth(max_chars) which gives index after max_chars chars
        assert_eq!(truncate(text, 3), "こんに...");
    }

    #[test]
    fn test_truncate_empty() {
        assert_eq!(truncate("", 5), "");
    }

    #[test]
    fn test_truncate_zero_limit() {
        assert_eq!(truncate("hello", 0), "...");
    }
}
