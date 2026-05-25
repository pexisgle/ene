use crate::config::WebSearchConfig;
use ene_tool_proto::{ToolCategory, ToolDefinition, ToolError};
use websearch::providers::{ArxivProvider, BraveProvider, DuckDuckGoProvider, ExaProvider, TavilyProvider};
use websearch::{web_search, SearchOptions, SearchProvider};

fn resolve_api_key(config: Option<&WebSearchConfig>, key: &str, env_var: &str) -> Result<String, ToolError> {
    if let Some(cfg) = config {
        match key {
            "tavily" if !cfg.tavily_api_key.is_empty() => return Ok(cfg.tavily_api_key.clone()),
            "brave" if !cfg.brave_api_key.is_empty() => return Ok(cfg.brave_api_key.clone()),
            "exa" if !cfg.exa_api_key.is_empty() => return Ok(cfg.exa_api_key.clone()),
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
            "Supports multiple search backends (ArXiv, DuckDuckGo, Tavily). ",
            "Returns summarized search results with titles, snippets, and URLs."
        )
        .to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "query": { "type": "string", "description": "The search query" },
                "backend": { "type": "string", "enum": ["arxiv", "brave", "duckduckgo", "exa", "tavily"], "description": "Search backend to use. Defaults to duckduckgo." },
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
        "arxiv" => Box::new(ArxivProvider::new()),
        "duckduckgo" => Box::new(DuckDuckGoProvider::new()),
        "tavily" => {
            let api_key = resolve_api_key(config, "tavily", "TAVILY_API_KEY")?;
            Box::new(TavilyProvider::new(&api_key).map_err(|e| {
                ToolError::ExecutionFailed {
                    message: format!("Tavily provider init failed: {e}"),
                }
            })?)
        }
        "brave" => {
            let api_key = resolve_api_key(config, "brave", "BRAVE_API_KEY")?;
            Box::new(BraveProvider::new(&api_key).map_err(|e| {
                ToolError::ExecutionFailed {
                    message: format!("Brave provider init failed: {e}"),
                }
            })?)
        }
        "exa" => {
            let api_key = resolve_api_key(config, "exa", "EXA_API_KEY")?;
            Box::new(ExaProvider::new(&api_key).map_err(|e| {
                ToolError::ExecutionFailed {
                    message: format!("Exa provider init failed: {e}"),
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
        "arxiv" => "ArXiv",
        "duckduckgo" => "DuckDuckGo",
        "tavily" => "Tavily",
        "brave" => "Brave",
        "exa" => "Exa",
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
