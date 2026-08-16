use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::search::error::SearchError;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    pub url: String,
    pub title: String,
    pub snippet: Option<String>,
    pub domain: Option<String>,
    pub published_date: Option<String>,
    pub provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw: Option<serde_json::Value>,
}

#[derive(Debug)]
pub struct SearchOptions {
    pub query: String,
    pub max_results: Option<u32>,
    pub provider: Box<dyn SearchProvider>,
}

impl Default for SearchOptions {
    fn default() -> Self {
        Self {
            query: String::new(),
            max_results: Some(10),
            provider: Box::new(DummyProvider),
        }
    }
}

#[async_trait]
pub trait SearchProvider: Send + Sync + std::fmt::Debug {
    #[expect(dead_code, reason = "implemented by each provider for diagnostics")]
    fn name(&self) -> &'static str;

    async fn search(&self, options: &SearchOptions) -> Result<Vec<SearchResult>, SearchError>;
}

#[derive(Debug)]
pub(crate) struct DummyProvider;

#[async_trait]
impl SearchProvider for DummyProvider {
    fn name(&self) -> &'static str {
        "dummy"
    }

    async fn search(&self, _options: &SearchOptions) -> Result<Vec<SearchResult>, SearchError> {
        Err(SearchError::InvalidInput(
            "No provider configured".to_string(),
        ))
    }
}
