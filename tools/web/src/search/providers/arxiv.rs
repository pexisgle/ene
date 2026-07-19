use async_trait::async_trait;
use serde::Deserialize;
use std::collections::HashMap;

use crate::search::error::SearchError;
use crate::search::types::{SearchOptions, SearchProvider, SearchResult};

#[derive(Debug, Deserialize)]
struct ArxivEntry {
    id: String,
    title: String,
    summary: String,
    published: String,
    #[serde(default)]
    authors: Vec<ArxivAuthor>,
    #[serde(default)]
    links: Vec<ArxivLink>,
}

#[derive(Debug, Deserialize)]
struct ArxivAuthor {
    name: String,
}

#[derive(Debug, Deserialize)]
struct ArxivLink {
    #[serde(rename = "href")]
    href: String,
    #[serde(rename = "type")]
    link_type: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ArxivFeed {
    #[serde(rename = "entry", default)]
    entries: Vec<ArxivEntry>,
}

/// `ArXiv` API provider.
#[derive(Debug, Default)]
pub struct ArxivProvider;

impl ArxivProvider {
    pub const fn new() -> Self {
        Self
    }
}

#[async_trait]
impl SearchProvider for ArxivProvider {
    fn name(&self) -> &'static str {
        "arxiv"
    }

    async fn search(&self, options: &SearchOptions) -> Result<Vec<SearchResult>, SearchError> {
        let client = reqwest::Client::new();
        let mut url = url::Url::parse("http://export.arxiv.org/api/query").map_err(|e| {
            SearchError::ConfigError(format!("Invalid ArXiv base URL: {e}"))
        })?;

        let search_query = format!("all:{}", options.query.trim());
        let max_results = options.max_results.unwrap_or(10).min(50);
        let max_results_str = max_results.to_string();

        {
            let mut pairs = url.query_pairs_mut();
            pairs.append_pair("search_query", &search_query);
            pairs.append_pair("max_results", &max_results_str);
        }

        let response = client.get(url).send().await.map_err(|e| SearchError::HttpError {
            message: format!("ArXiv API request failed: {e}"),
            status_code: None,
            response_body: None,
        })?;

        let status = response.status();
        let xml_text = response.text().await.map_err(|e| SearchError::HttpError {
            message: format!("ArXiv response read failed: {e}"),
            status_code: Some(status.as_u16()),
            response_body: None,
        })?;

        if !status.is_success() {
            return Err(SearchError::HttpError {
                message: format!("ArXiv API returned error: {status}"),
                status_code: Some(status.as_u16()),
                response_body: Some(xml_text),
            });
        }

        let feed: ArxivFeed = quick_xml::de::from_str(&xml_text).map_err(|e| {
            SearchError::ParseError(format!("Failed to parse ArXiv XML: {e}"))
        })?;

        Ok(feed
            .entries
            .into_iter()
            .map(|entry| {
                let arxiv_id = entry
                    .id
                    .rsplit('/')
                    .next()
                    .unwrap_or(&entry.id)
                    .to_string();
                let paper_url = entry
                    .links
                    .iter()
                    .find(|link| link.link_type.as_deref() == Some("text/html"))
                    .map_or_else(
                        || format!("https://arxiv.org/abs/{arxiv_id}"),
                        |link| link.href.clone(),
                    );
                let authors: Vec<String> = entry.authors.iter().map(|a| a.name.clone()).collect();
                let authors_string = if authors.is_empty() {
                    None
                } else {
                    Some(authors.join(", "))
                };
                let mut raw_data = HashMap::new();
                raw_data.insert(
                    "arxiv_id".to_string(),
                    serde_json::Value::String(arxiv_id.clone()),
                );
                raw_data.insert(
                    "published".to_string(),
                    serde_json::Value::String(entry.published.clone()),
                );
                if let Some(authors_str) = &authors_string {
                    raw_data.insert(
                        "authors".to_string(),
                        serde_json::Value::String(authors_str.clone()),
                    );
                }

                SearchResult {
                    url: paper_url,
                    title: entry.title.trim().to_string(),
                    snippet: Some(entry.summary.trim().to_string()),
                    domain: Some("arxiv.org".to_string()),
                    published_date: Some(entry.published),
                    provider: Some("arxiv".to_string()),
                    raw: Some(serde_json::to_value(raw_data).unwrap_or_default()),
                }
            })
            .collect())
    }
}
