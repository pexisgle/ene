use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::time::Duration;

use crate::search::error::SearchError;
use crate::search::types::{SearchOptions, SearchProvider, SearchResult};

use super::extract_domain;

#[derive(Debug, Deserialize, Serialize)]
struct TavilySearchResult {
    title: String,
    url: String,
    content: String,
    published_date: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TavilyResponse {
    results: Vec<TavilySearchResult>,
}

#[derive(Debug, Serialize)]
struct TavilyRequest {
    api_key: String,
    query: String,
    search_depth: String,
    include_answer: bool,
    include_images: bool,
    include_raw_content: bool,
    max_results: u32,
}

/// Tavily Search API provider.
#[derive(Debug, Clone)]
pub struct TavilyProvider {
    api_key: String,
}

impl TavilyProvider {
    pub fn new(api_key: &str) -> Result<Self, SearchError> {
        if api_key.is_empty() {
            return Err(SearchError::ConfigError(
                "Tavily API key is required".to_string(),
            ));
        }
        if !api_key.starts_with("tvly-") {
            return Err(SearchError::ConfigError(
                "Invalid Tavily API key format. Keys should start with 'tvly-'".to_string(),
            ));
        }

        Ok(Self {
            api_key: api_key.to_string(),
        })
    }
}

#[async_trait]
impl SearchProvider for TavilyProvider {
    fn name(&self) -> &'static str {
        "tavily"
    }

    async fn search(&self, options: &SearchOptions) -> Result<Vec<SearchResult>, SearchError> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|e| SearchError::ConfigError(format!("HTTP client init failed: {e}")))?;

        let max_results = options.max_results.unwrap_or(10).min(50);
        let request_body = TavilyRequest {
            api_key: self.api_key.clone(),
            query: options.query.clone(),
            search_depth: "basic".to_string(),
            include_answer: true,
            include_images: false,
            include_raw_content: false,
            max_results,
        };

        let response = client
            .post("https://api.tavily.com/search")
            .header("Content-Type", "application/json")
            .json(&request_body)
            .send()
            .await
            .map_err(|e| SearchError::HttpError {
                message: format!("Tavily request failed: {e}"),
                status_code: None,
                response_body: None,
            })?;

        let status = response.status();
        let response_text = response.text().await.map_err(|e| SearchError::HttpError {
            message: format!("Tavily response read failed: {e}"),
            status_code: Some(status.as_u16()),
            response_body: None,
        })?;

        if !status.is_success() {
            return Err(SearchError::HttpError {
                message: format!("Tavily API error ({status})"),
                status_code: Some(status.as_u16()),
                response_body: Some(response_text),
            });
        }

        let tavily_response: TavilyResponse =
            serde_json::from_str(&response_text).map_err(|e| {
                SearchError::ParseError(format!("Failed to parse Tavily response: {e}"))
            })?;

        Ok(tavily_response
            .results
            .into_iter()
            .map(|result| SearchResult {
                url: result.url.clone(),
                title: result.title,
                snippet: Some(result.content),
                domain: extract_domain(&result.url),
                published_date: result.published_date,
                provider: Some("tavily".to_string()),
                raw: None,
            })
            .collect())
    }
}
