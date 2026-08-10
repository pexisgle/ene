use async_trait::async_trait;
use ene_plugin_broker::HttpMethod;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

use crate::broker::WebBroker;
use crate::search::error::SearchError;
use crate::search::types::{SearchOptions, SearchProvider, SearchResult};

use super::extract_domain;

#[derive(Debug, Deserialize)]
struct ExaSearchResult {
    title: String,
    url: String,
    text: Option<String>,
    #[serde(rename = "publishedDate")]
    published_date: Option<String>,
    author: Option<String>,
    score: Option<f64>,
}

#[derive(Debug, Deserialize)]
struct ExaSearchResponse {
    results: Vec<ExaSearchResult>,
}

#[derive(Debug, Serialize)]
struct ExaSearchRequest {
    query: String,
    #[serde(rename = "max_results")]
    max_results: Option<usize>,
    model: String,
    #[serde(rename = "include_contents")]
    include_contents: bool,
}

#[derive(Debug)]
pub struct ExaProvider {
    credential: String,
    broker: Arc<WebBroker>,
}

impl ExaProvider {
    pub fn new(credential: &str, broker: Arc<WebBroker>) -> Result<Self, SearchError> {
        if credential.is_empty() {
            return Err(SearchError::ConfigError(
                "Exa credential name is required".to_string(),
            ));
        }

        Ok(Self {
            credential: credential.to_string(),
            broker,
        })
    }
}

#[async_trait]
impl SearchProvider for ExaProvider {
    fn name(&self) -> &'static str {
        "exa"
    }

    async fn search(&self, options: &SearchOptions) -> Result<Vec<SearchResult>, SearchError> {
        let request_body = ExaSearchRequest {
            query: options.query.clone(),
            max_results: options.max_results.map(|n| n as usize),
            model: "keyword".to_string(),
            include_contents: false,
        };

        let body = serde_json::to_vec(&request_body).map_err(|e| {
            SearchError::ProviderError(format!("Exa request serialization failed: {e}"))
        })?;
        let response = self
            .broker
            .fetch(
                HttpMethod::Post,
                "https://api.exa.ai/search",
                vec![("Content-Type".to_string(), "application/json".to_string())],
                Some(body),
                5 * 1024 * 1024,
                Some(&self.credential),
                Some("x-api-key"),
            )
            .await
            .map_err(|e| SearchError::ProviderError(format!("Exa API request failed: {e}")))?;

        let status = response.status;
        if !(200..300).contains(&status) {
            let error_text = String::from_utf8_lossy(&response.body).into_owned();
            return Err(SearchError::ProviderError(format!(
                "Exa API request failed (HTTP {status}): {error_text}"
            )));
        }

        let exa_response: ExaSearchResponse =
            serde_json::from_slice(&response.body).map_err(|e| {
                SearchError::ProviderError(format!("Failed to parse Exa response: {e}"))
            })?;

        Ok(exa_response
            .results
            .into_iter()
            .map(|result| {
                let domain = extract_domain(&result.url);
                let mut raw_data = HashMap::new();
                if let Some(score) = result.score {
                    raw_data.insert(
                        "score".to_string(),
                        serde_json::Value::Number(
                            serde_json::Number::from_f64(score)
                                .unwrap_or_else(|| serde_json::Number::from(0)),
                        ),
                    );
                }
                if let Some(author) = &result.author {
                    raw_data.insert(
                        "author".to_string(),
                        serde_json::Value::String(author.clone()),
                    );
                }

                SearchResult {
                    url: result.url,
                    title: result.title,
                    snippet: result.text,
                    domain,
                    published_date: result.published_date,
                    provider: Some("exa".to_string()),
                    raw: if raw_data.is_empty() {
                        None
                    } else {
                        Some(serde_json::to_value(raw_data).unwrap_or_default())
                    },
                }
            })
            .collect())
    }
}
