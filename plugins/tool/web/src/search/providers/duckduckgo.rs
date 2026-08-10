use async_trait::async_trait;
use ene_plugin_broker::HttpMethod;
use scraper::{Html, Selector};
use std::sync::Arc;

use crate::broker::WebBroker;
use crate::search::error::SearchError;
use crate::search::types::{SearchOptions, SearchProvider, SearchResult};

use super::extract_domain;

#[derive(Debug)]
pub struct DuckDuckGoProvider {
    broker: Arc<WebBroker>,
}

impl DuckDuckGoProvider {
    pub fn new(broker: Arc<WebBroker>) -> Self {
        Self { broker }
    }
}

#[async_trait]
impl SearchProvider for DuckDuckGoProvider {
    fn name(&self) -> &'static str {
        "duckduckgo"
    }

    async fn search(&self, options: &SearchOptions) -> Result<Vec<SearchResult>, SearchError> {
        let body = format!("q={}&b=&kl=wt-wt", percent_encode(&options.query));
        let response = self
            .broker
            .fetch(
                HttpMethod::Post,
                "https://html.duckduckgo.com/html",
                vec![
                    (
                        "Referer".to_string(),
                        "https://html.duckduckgo.com/".to_string(),
                    ),
                    (
                        "Content-Type".to_string(),
                        "application/x-www-form-urlencoded".to_string(),
                    ),
                ],
                Some(body.into_bytes()),
                5 * 1024 * 1024,
                None,
                None,
            )
            .await
            .map_err(|e| SearchError::HttpError {
                message: format!("DuckDuckGo request failed: {e}"),
                status_code: None,
                response_body: None,
            })?;

        let status = response.status;
        let html = String::from_utf8_lossy(&response.body).into_owned();

        if !(200..300).contains(&status) {
            return Err(SearchError::HttpError {
                message: "DuckDuckGo returned an error".to_string(),
                status_code: Some(status),
                response_body: Some(html),
            });
        }

        parse_text_results(&html, options.max_results.unwrap_or(10))
    }
}

fn percent_encode(input: &str) -> String {
    // Form-encoding for the DuckDuckGo query: percent-encode everything
    // except unreserved characters.
    let mut out = String::new();
    for byte in input.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(char::from(*byte));
            }
            _ => {
                use std::fmt::Write;
                #[expect(
                    clippy::let_underscore_must_use,
                    reason = "fmt::Write to a String is infallible in practice"
                )]
                let _ = write!(out, "%{byte:02X}");
            }
        }
    }
    out
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
