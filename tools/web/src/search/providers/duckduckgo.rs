use async_trait::async_trait;
use scraper::{Html, Selector};

use crate::search::error::SearchError;
use crate::search::types::{SearchOptions, SearchProvider, SearchResult};

use super::extract_domain;

/// `DuckDuckGo` HTML search provider.
#[derive(Debug)]
pub struct DuckDuckGoProvider {
    client: reqwest::Client,
}

impl DuckDuckGoProvider {
    pub fn new(client: reqwest::Client) -> Self {
        Self { client }
    }
}

#[async_trait]
impl SearchProvider for DuckDuckGoProvider {
    fn name(&self) -> &'static str {
        "duckduckgo"
    }

    async fn search(&self, options: &SearchOptions) -> Result<Vec<SearchResult>, SearchError> {
        let response = self
            .client
            .post("https://html.duckduckgo.com/html")
            .header("Referer", "https://html.duckduckgo.com/")
            .form(&[("q", options.query.as_str()), ("b", ""), ("kl", "wt-wt")])
            .send()
            .await
            .map_err(|e| SearchError::HttpError {
                message: format!("DuckDuckGo request failed: {e}"),
                status_code: None,
                response_body: None,
            })?;

        let status = response.status();
        let html = response.text().await.map_err(|e| SearchError::HttpError {
            message: format!("DuckDuckGo response read failed: {e}"),
            status_code: Some(status.as_u16()),
            response_body: None,
        })?;

        if !status.is_success() {
            return Err(SearchError::HttpError {
                message: "DuckDuckGo returned an error".to_string(),
                status_code: Some(status.as_u16()),
                response_body: Some(html),
            });
        }

        parse_text_results(&html, options.max_results.unwrap_or(10))
    }
}

fn parse_text_results(html: &str, max_results: u32) -> Result<Vec<SearchResult>, SearchError> {
    let document = Html::parse_document(html);
    let result_selector = Selector::parse("h2.result__title a").map_err(|_| {
        SearchError::ParseError("Invalid CSS selector for DuckDuckGo results".to_string())
    })?;
    let snippet_selector = Selector::parse(".result__snippet").map_err(|_| {
        SearchError::ParseError("Invalid CSS selector for DuckDuckGo snippets".to_string())
    })?;

    let result_links: Vec<_> = document.select(&result_selector).collect();
    let result_snippets: Vec<_> = document.select(&snippet_selector).collect();
    let mut results = Vec::new();

    for (i, link_element) in result_links.iter().enumerate() {
        if results.len() >= max_results as usize {
            break;
        }

        let Some(href) = link_element.value().attr("href") else {
            continue;
        };
        if href.contains("duckduckgo.com") || href.contains("google.com/search") {
            continue;
        }

        let url = normalize_url(href);
        let title = normalize_text(&link_element.inner_html());
        let snippet = result_snippets
            .get(i)
            .map(|snippet_elem| normalize_text(&snippet_elem.inner_html()));
        let domain = extract_domain(&url);

        results.push(SearchResult {
            url,
            title,
            snippet,
            domain,
            published_date: None,
            provider: Some("duckduckgo".to_string()),
            raw: None,
        });
    }

    Ok(results)
}

fn normalize_url(href: &str) -> String {
    if let Some(stripped) = href.strip_prefix("//") {
        format!("https://{stripped}")
    } else {
        href.to_string()
    }
}

fn normalize_text(html: &str) -> String {
    Html::parse_fragment(html)
        .root_element()
        .text()
        .collect::<Vec<_>>()
        .join("")
        .trim()
        .to_string()
}
