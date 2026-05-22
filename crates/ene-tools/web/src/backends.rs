use ene_tool_proto::ToolError;
use std::sync::LazyLock;
use regex::Regex;

static RE_HTML_TAG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<[^>]+>").unwrap());
static RE_DDG_BODY: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"<div class="result__body"[^>]*>.*?<a[^>]*class="result__a"[^>]*href="([^"]*)"[^>]*>(.*?)</a>.*?<a[^>]*class="result__snippet"[^>]*>(.*?)</a>.*?</div>"#).unwrap()
});
static RE_DDG_ALT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"<h[^>]*>.*?<a[^>]*href="([^"]*)"[^>]*>(.*?)</a>.*?</h[^>]*>.*?<p[^>]*>(.*?)</p>"#).unwrap()
});

pub struct SearchResult {
    pub title: String,
    pub snippet: String,
    pub url: String,
}

pub async fn search_duckduckgo(client: &reqwest::Client, query: &str, limit: usize) -> Result<String, ToolError> {
    let url = format!(
        "https://html.duckduckgo.com/html/?q={}",
        urlencoding::encode(query)
    );

    let response = client
        .get(&url)
        .send()
        .await
        .map_err(|e| ToolError::ExecutionFailed {
            message: format!("Request failed: {e}"),
        })?;

    let html = response
        .text()
        .await
        .map_err(|e| ToolError::ExecutionFailed {
            message: format!("Read failed: {e}"),
        })?;

    let results = parse_duckduckgo_html(&html, limit);
    if results.is_empty() {
        return Ok("No results found.".to_string());
    }

    let mut output = format!("Search results for '{}' (DuckDuckGo):\n\n", query);
    for (i, result) in results.iter().enumerate() {
        output.push_str(&format!(
            "{}. {}\n   {}\n   URL: {}\n\n",
            i + 1,
            result.title,
            result.snippet,
            result.url
        ));
    }
    Ok(output)
}

pub async fn search_tavily(client: &reqwest::Client, query: &str, _limit: usize) -> Result<String, ToolError> {
    let api_key = std::env::var("TAVILY_API_KEY").map_err(|_| ToolError::ExecutionFailed {
        message: "TAVILY_API_KEY environment variable is not set".to_string(),
    })?;

    let response = client
        .post("https://api.tavily.com/search")
        .header("Content-Type", "application/json")
        .json(&serde_json::json!({
            "api_key": api_key,
            "query": query,
        }))
        .send()
        .await
        .map_err(|e| ToolError::ExecutionFailed {
            message: format!("Request failed: {e}"),
        })?;

    let json: serde_json::Value =
        response
            .json()
            .await
            .map_err(|e| ToolError::ExecutionFailed {
                message: format!("Parse failed: {e}"),
            })?;

    let results = json["results"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|r| {
                    Some(SearchResult {
                        title: r["title"].as_str()?.to_string(),
                        snippet: r["content"].as_str()?.to_string(),
                        url: r["url"].as_str()?.to_string(),
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    if results.is_empty() {
        return Ok("No results found.".to_string());
    }

    let mut output = format!("Search results for '{}' (Tavily):\n\n", query);
    for (i, result) in results.iter().enumerate() {
        output.push_str(&format!(
            "{}. {}\n   {}\n   URL: {}\n\n",
            i + 1,
            result.title,
            result.snippet,
            result.url
        ));
    }
    Ok(output)
}

pub async fn search_brave(client: &reqwest::Client, query: &str, _limit: usize) -> Result<String, ToolError> {
    let api_key = std::env::var("BRAVE_API_KEY").map_err(|_| ToolError::ExecutionFailed {
        message: "BRAVE_API_KEY environment variable is not set".to_string(),
    })?;

    let response = client
        .get("https://api.search.brave.com/res/v1/web/search")
        .header("X-Subscription-Token", &api_key)
        .query(&[("q", query)])
        .send()
        .await
        .map_err(|e| ToolError::ExecutionFailed {
            message: format!("Request failed: {e}"),
        })?;

    let json: serde_json::Value =
        response
            .json()
            .await
            .map_err(|e| ToolError::ExecutionFailed {
                message: format!("Parse failed: {e}"),
            })?;

    let results = json["web"]["results"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|r| {
                    Some(SearchResult {
                        title: r["title"].as_str()?.to_string(),
                        snippet: r["description"].as_str()?.to_string(),
                        url: r["url"].as_str()?.to_string(),
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    if results.is_empty() {
        return Ok("No results found.".to_string());
    }

    let mut output = format!("Search results for '{}' (Brave):\n\n", query);
    for (i, result) in results.iter().enumerate() {
        output.push_str(&format!(
            "{}. {}\n   {}\n   URL: {}\n\n",
            i + 1,
            result.title,
            result.snippet,
            result.url
        ));
    }
    Ok(output)
}

fn parse_duckduckgo_html(html: &str, limit: usize) -> Vec<SearchResult> {
    let mut results = Vec::new();

    for cap in RE_DDG_BODY.captures_iter(html) {
        if results.len() >= limit {
            break;
        }
        let url = html_unescape(&cap[1]);
        let title = html_unescape(&cap[2]);
        let snippet = html_unescape(&cap[3]);
        results.push(SearchResult {
            title,
            snippet,
            url,
        });
    }

    if results.is_empty() {
        for cap in RE_DDG_ALT.captures_iter(html) {
            if results.len() >= limit {
                break;
            }
            let url = html_unescape(&cap[1]);
            let title = html_unescape(&cap[2]);
            let snippet = html_unescape(&cap[3]);
            results.push(SearchResult {
                title,
                snippet,
                url,
            });
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
    result = RE_HTML_TAG.replace_all(&result, "").to_string();
    result.trim().to_string()
}
