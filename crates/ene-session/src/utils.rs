use ene_config::AiSettings;
use ene_embedding::{EmbeddingProvider, create_embedding_provider};

use ene_memory::MemoryStore;
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

/// Embedding プロバイダーを初期化する（常に利用可能）
pub fn init_embedding(
    settings: &AiSettings,
) -> Result<Arc<dyn EmbeddingProvider>, String> {
    let embed_base_url = settings
        .resolve_embedding_base_url()
        .map_err(|e| format!("Failed to resolve embedding base URL: {}", e))?;

    let embedder = create_embedding_provider(
        settings.embedding.provider_type,
        &settings.embedding.model,
        &embed_base_url,
        &settings.resolve_api_key(),
        settings.embedding.dimensions.unwrap_or(768),
        Some(&settings.embedding.gguf_quantization),
    )
    .map_err(|e| format!("Failed to create embedding provider: {}", e))?;

    Ok(Arc::from(embedder))
}

/// MemoryStore を初期化する（EmbeddingProvider が必要）
pub fn init_memory_store(
    settings: &AiSettings,
    embedder: &dyn EmbeddingProvider,
) -> Result<Arc<MemoryStore>, String> {
    let db_path = settings.resolve_memory_db_path();

    if let Some(parent) = db_path.parent() {
        if !parent.exists() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create memory DB directory: {}", e))?;
        }
    }

    let dims = embedder.dimensions();

    let store = MemoryStore::open(&db_path, dims)
        .map_err(|e| format!("Failed to open memory store: {}", e))?;

    Ok(Arc::new(store))
}

/// メモリ機能を初期化する（EmbeddingProvider + MemoryStore）
pub fn init_memory(
    settings: &AiSettings,
) -> Result<(Arc<MemoryStore>, Arc<dyn EmbeddingProvider>), String> {
    let embedder = init_embedding(settings)?;
    let store = init_memory_store(settings, &*embedder)?;
    Ok((store, embedder))
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
