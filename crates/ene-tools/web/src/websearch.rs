use crate::backends;
use ene_tool_proto::{ToolCategory, ToolDefinition, ToolError};

pub fn tool_definition() -> ToolDefinition {
    ToolDefinition {
        name: "websearch".to_string(),
        description: concat!(
            "Searches the web for latest information and technical references. ",
            "Supports multiple search backends (DuckDuckGo, Tavily, Brave). ",
            "Returns summarized search results with titles, snippets, and URLs."
        )
        .to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "query": { "type": "string", "description": "The search query" },
                "backend": { "type": "string", "enum": ["duckduckgo", "tavily", "brave"], "description": "Search backend to use. Defaults to duckduckgo." },
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
    client: &reqwest::Client,
    query: &str,
    backend: Option<&str>,
    limit: Option<usize>,
) -> Result<String, ToolError> {
    let backend_name = backend.unwrap_or("duckduckgo");
    let limit = limit.unwrap_or(5).min(10);

    match backend_name {
        "duckduckgo" => backends::search_duckduckgo(client, query, limit).await,
        "tavily" => backends::search_tavily(client, query, limit).await,
        "brave" => backends::search_brave(client, query, limit).await,
        _ => Err(ToolError::InvalidArguments {
            message: format!("Unknown backend: {backend_name}"),
        }),
    }
}
