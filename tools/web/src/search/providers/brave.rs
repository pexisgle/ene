use async_trait::async_trait;

use crate::search::error::SearchError;
use crate::search::types::{SearchOptions, SearchProvider, SearchResult};

/// Brave Search API provider (not yet implemented).
#[derive(Debug, Clone)]
pub struct BraveProvider {
    #[expect(dead_code, reason = "stored for future Brave API integration")]
    api_key: String,
}

impl BraveProvider {
    pub fn new(api_key: &str) -> Result<Self, SearchError> {
        if api_key.is_empty() {
            return Err(SearchError::ConfigError(
                "Brave API key is required".to_string(),
            ));
        }

        Ok(Self {
            api_key: api_key.to_string(),
        })
    }
}

#[async_trait]
impl SearchProvider for BraveProvider {
    fn name(&self) -> &'static str {
        "brave"
    }

    async fn search(&self, _options: &SearchOptions) -> Result<Vec<SearchResult>, SearchError> {
        Err(SearchError::ProviderError(
            "Brave provider implementation coming soon".to_string(),
        ))
    }
}
