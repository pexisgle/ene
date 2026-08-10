use async_trait::async_trait;
use ene_plugin_broker::HttpMethod;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::broker::WebBroker;
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
    query: String,
    search_depth: String,
    include_answer: bool,
    include_images: bool,
    include_raw_content: bool,
    max_results: u32,
}

#[derive(Debug)]
pub struct TavilyProvider {
    credential: String,
    broker: Arc<WebBroker>,
}

impl TavilyProvider {
    pub fn new(credential: &str, broker: Arc<WebBroker>) -> Result<Self, SearchError> {
        if credential.is_empty() {
            return Err(SearchError::ConfigError(
                "Tavily credential name is required".to_string(),
            ));
        }

        Ok(Self {
            credential: credential.to_string(),
            broker,
        })
    }
}

#[async_trait]
impl SearchProvider for TavilyProvider {
    fn name(&self) -> &'static str {
        "tavily"
    }

    async fn search(&self, options: &SearchOptions) -> Result<Vec<SearchResult>, SearchError> {
        let max_results = options.max_results.unwrap_or(10).min(50);
        let request_body = TavilyRequest {
            query: options.query.clone(),
            search_depth: "basic".to_string(),
            include_answer: true,
            include_images: false,
            include_raw_content: false,
            max_results,
        };

        let body = serde_json::to_vec(&request_body).map_err(|e| SearchError::HttpError {
            message: format!("Tavily request serialization failed: {e}"),
            status_code: None,
            response_body: None,
        })?;
        let response = self
            .broker
            .fetch(
                HttpMethod::Post,
                "https://api.tavily.com/search",
                vec![("Content-Type".to_string(), "application/json".to_string())],
                Some(body),
                5 * 1024 * 1024,
                Some(&self.credential),
                None,
            )
            .await
            .map_err(|e| SearchError::HttpError {
                message: format!("Tavily request failed: {e}"),
                status_code: None,
                response_body: None,
            })?;

        let status = response.status;
        let response_text = String::from_utf8_lossy(&response.body).into_owned();

        if !(200..300).contains(&status) {
            return Err(SearchError::HttpError {
                message: format!("Tavily API error (HTTP {status})"),
                status_code: Some(status),
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
