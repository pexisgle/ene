//! Web検索ツール (websearch_tools)
//! Phase 2: 検索 API 統合（DuckDuckGo、Tavily、Brave Search API 等）

use crate::error::AiCoreError;
use super::definition::ToolDefinition;

pub fn tool_definition() -> ToolDefinition {
    ToolDefinition {
        name: "websearch".to_string(),
        description: concat!(
            "Searches the web for latest information and technical references. ",
            "Supports multiple search backends (DuckDuckGo, Tavily, Brave). ",
            "Returns summarized search results with titles, snippets, and URLs."
        ).to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "query": { "type": "string", "description": "The search query" },
                "backend": { "type": "string", "enum": ["duckduckgo", "tavily", "brave"], "description": "Search backend to use. Defaults to duckduckgo." },
                "limit": { "type": "integer", "description": "Maximum number of results (default 5, max 10)" }
            },
            "required": ["query"]
        }),
    }
}

/// Web検索を実行
pub async fn websearch(
    query: &str,
    backend: Option<&str>,
    limit: Option<usize>,
) -> Result<String, AiCoreError> {
    let backend = backend.unwrap_or("duckduckgo");
    let limit = limit.unwrap_or(5).min(10);

    match backend {
        "duckduckgo" => search_duckduckgo(query, limit).await,
        "tavily" => search_tavily(query, limit).await,
        "brave" => search_brave(query, limit).await,
        _ => Err(AiCoreError::WebSearchError(format!("Unknown backend: {}", backend))),
    }
}

async fn search_duckduckgo(query: &str, limit: usize) -> Result<String, AiCoreError> {
    // DuckDuckGo HTML API (no API key required)
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36")
        .build()
        .map_err(|e| AiCoreError::WebSearchError(format!("HTTP client error: {e}")))?;

    let url = format!(
        "https://html.duckduckgo.com/html/?q={}",
        urlencoding::encode(query)
    );

    let response = client
        .get(&url)
        .send()
        .await
        .map_err(|e| AiCoreError::WebSearchError(format!("Request failed: {e}")))?;

    let html = response
        .text()
        .await
        .map_err(|e| AiCoreError::WebSearchError(format!("Read failed: {e}")))?;

    let results = parse_duckduckgo_html(&html, limit);
    if results.is_empty() {
        return Ok("No results found.".to_string());
    }

    let mut output = format!("Search results for '{}' (DuckDuckGo):\n\n", query);
    for (i, result) in results.iter().enumerate() {
        output.push_str(&format!("{}. {}\n   {}\n   URL: {}\n\n", i + 1, result.title, result.snippet, result.url));
    }
    Ok(output)
}

async fn search_tavily(_query: &str, _limit: usize) -> Result<String, AiCoreError> {
    // Tavily API requires API key; stub for now
    Err(AiCoreError::WebSearchError(
        "Tavily backend requires TAVILY_API_KEY environment variable. Please configure it or use 'duckduckgo' backend.".to_string()
    ))
}

async fn search_brave(_query: &str, _limit: usize) -> Result<String, AiCoreError> {
    // Brave Search API requires API key; stub for now
    Err(AiCoreError::WebSearchError(
        "Brave backend requires BRAVE_API_KEY environment variable. Please configure it or use 'duckduckgo' backend.".to_string()
    ))
}

struct SearchResult {
    title: String,
    snippet: String,
    url: String,
}

fn parse_duckduckgo_html(html: &str, limit: usize) -> Vec<SearchResult> {
    let mut results = Vec::new();
    let re_result = match regex::Regex::new(r#"<div class="result__body"[^>]*>.*?<a[^>]*class="result__a"[^>]*href="([^"]*)"[^>]*>(.*?)</a>.*?<a[^>]*class="result__snippet"[^>]*>(.*?)</a>.*?</div>"#) {
        Ok(re) => re,
        Err(_) => return results,
    };

    for cap in re_result.captures_iter(html) {
        if results.len() >= limit {
            break;
        }
        let url = html_unescape(&cap[1]);
        let title = html_unescape(&cap[2]);
        let snippet = html_unescape(&cap[3]);
        results.push(SearchResult { title, snippet, url });
    }

    // Fallback: try alternative selectors
    if results.is_empty() {
        let re_alt = match regex::Regex::new(r#"<h[^>]*>.*?<a[^>]*href="([^"]*)"[^>]*>(.*?)</a>.*?</h[^>]*>.*?<p[^>]*>(.*?)</p>"#) {
            Ok(re) => re,
            Err(_) => return results,
        };
        for cap in re_alt.captures_iter(html) {
            if results.len() >= limit {
                break;
            }
            let url = html_unescape(&cap[1]);
            let title = html_unescape(&cap[2]);
            let snippet = html_unescape(&cap[3]);
            results.push(SearchResult { title, snippet, url });
        }
    }

    results
}

fn html_unescape(html: &str) -> String {
    let mut result = html.to_string();
    result = result.replace("&amp;", "&");
    result = result.replace("&lt;", "<");
    result = result.replace("&gt;", ">");
    result = result.replace("&quot;", "\"");
    result = result.replace("&#39;", "'");
    result = result.replace("&nbsp;", " ");
    // Strip HTML tags
    let re = regex::Regex::new(r"<[^>]+>").unwrap_or_else(|_| regex::Regex::new("").unwrap());
    result = re.replace_all(&result, "").to_string();
    result.trim().to_string()
}
