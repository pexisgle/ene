use crate::WebSearchConfig;
use ene_tool_proto::{ToolCategory, ToolDefinition, ToolError};
use websearch::providers::{DuckDuckGoProvider, GoogleProvider, TavilyProvider};
use websearch::{web_search, SearchOptions, SearchProvider};

fn resolve_api_key(config: Option<&WebSearchConfig>, key: &str, env_var: &str) -> Result<String, ToolError> {
    if let Some(cfg) = config {
        match key {
            "tavily" if !cfg.tavily_api_key.is_empty() => return Ok(cfg.tavily_api_key.clone()),
            "google_key" if !cfg.google_api_key.is_empty() => return Ok(cfg.google_api_key.clone()),
            "google_cx" if !cfg.google_cse_cx.is_empty() => return Ok(cfg.google_cse_cx.clone()),
            _ => {}
        }
    }
    std::env::var(env_var).map_err(|_| ToolError::ExecutionFailed {
        message: format!("{env_var} not set. Set it in settings.json (tools.web.{key}_api_key) or as environment variable {env_var}."),
    })
}

pub fn tool_definition() -> ToolDefinition {
    ToolDefinition {
        name: "websearch".to_string(),
        description: concat!(
            "Searches the web for latest information and technical references. ",
            "Supports multiple search backends (DuckDuckGo, Tavily, Google). ",
            "Returns summarized search results with titles, snippets, and URLs."
        )
        .to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "query": { "type": "string", "description": "The search query" },
                "backend": { "type": "string", "enum": ["duckduckgo", "tavily", "google"], "description": "Search backend to use. Defaults to duckduckgo." },
                "limit": { "type": "integer", "description": "Maximum number of results (default 5, max 10)" }
            },
            "required": ["query"]
        }),
        category: Some(ToolCategory::WebSearch),
        keywords: vec![
            "search".to_string(),
            "web".to_string(),
            "google".to_string(),
            "internet".to_string(),
            "lookup".to_string(),
        ],
    }
}

pub async fn websearch(
    query: &str,
    backend: Option<&str>,
    limit: Option<usize>,
    config: Option<&WebSearchConfig>,
) -> Result<String, ToolError> {
    let backend_name = backend.unwrap_or("duckduckgo");
    let limit = limit.unwrap_or(5).min(10);

    let provider: Box<dyn SearchProvider> = match backend_name {
        "duckduckgo" => Box::new(DuckDuckGoProvider::new()),
        "tavily" => {
            let api_key = resolve_api_key(config, "tavily", "TAVILY_API_KEY")?;
            Box::new(TavilyProvider::new(&api_key).map_err(|e| {
                ToolError::ExecutionFailed {
                    message: format!("Tavily provider init failed: {e}"),
                }
            })?)
        }
        "google" => {
            let api_key = resolve_api_key(config, "google_key", "GOOGLE_API_KEY")?;
            let cx = resolve_api_key(config, "google_cx", "GOOGLE_CSE_CX")?;
            Box::new(GoogleProvider::new(&api_key, &cx).map_err(|e| {
                ToolError::ExecutionFailed {
                    message: format!("Google provider init failed: {e}"),
                }
            })?)
        }
        _ => {
            return Err(ToolError::InvalidArguments {
                message: format!("Unknown backend: {backend_name}"),
            })
        }
    };

    let results = web_search(SearchOptions {
        query: query.to_string(),
        max_results: Some(limit as u32),
        provider,
        ..Default::default()
    })
    .await
    .map_err(|e| ToolError::ExecutionFailed {
        message: format!("Search failed: {e}"),
    })?;

    if results.is_empty() {
        return Ok("No results found.".to_string());
    }

    let provider_label = match backend_name {
        "duckduckgo" => "DuckDuckGo",
        "tavily" => "Tavily",
        "google" => "Google",
        _ => backend_name,
    };

    let mut output = format!("Search results for '{query}' ({provider_label}):\n\n");
    for (i, result) in results.iter().enumerate() {
        let snippet = result.snippet.as_deref().unwrap_or("");
        output.push_str(&format!(
            "{}. {}\n   {snippet}\n   URL: {}\n\n",
            i + 1,
            result.title,
            result.url,
        ));
    }
    Ok(output)
}
