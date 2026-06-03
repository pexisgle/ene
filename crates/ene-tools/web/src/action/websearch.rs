use crate::provider::WebSearchConfig;
use async_trait::async_trait;
use ene_tool_proto::{
    KeywordSet, SideEffects, ToolCategory, ToolError, ToolExample, ToolName, ToolSpec, ToolVersion,
};
use ene_tools_common::ToolAction;
use serde::Deserialize;
use std::sync::{Arc, OnceLock};
use websearch::providers::{
    ArxivProvider, BraveProvider, DuckDuckGoProvider, ExaProvider, TavilyProvider,
};
use websearch::{SearchOptions, SearchProvider, web_search};

#[derive(Deserialize)]
struct WebSearchArgs {
    query: String,
    #[serde(default)]
    backend: Option<String>,
    #[serde(default)]
    limit: Option<usize>,
}

fn resolve_api_key(
    config: Option<&WebSearchConfig>,
    key: &str,
    env_var: &str,
) -> Result<String, ToolError> {
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

/// Action to search the web for latest information.
pub struct WebSearchAction {
    config: Arc<OnceLock<WebSearchConfig>>,
}

impl WebSearchAction {
    /// Creates a new `WebSearchAction` with a reference to the shared configuration OnceLock.
    pub fn new(config: Arc<OnceLock<WebSearchConfig>>) -> Self {
        Self { config }
    }
}

#[async_trait]
impl ToolAction for WebSearchAction {
    fn tool_name(&self) -> &'static str {
        "web.search"
    }

    fn definition(&self) -> ToolSpec {
        ToolSpec {
            name: ToolName::new("web.search"),
            version: ToolVersion::default(),
            display_name: "Search the web for the latest information.".to_string(),
            summary: "Search the web for the latest information.".to_string(),
            description: "Searches the web using the configured backend (duckduckgo, tavily, brave, exa, or arxiv) and returns a list of relevant results with titles, URLs, and snippets.".to_string(),
            category: ToolCategory::WebSearch,
            keywords: KeywordSet::primary_only(["search", "web", "google", "internet", "lookup"]),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "The search query" },
                    "backend": { "type": "string", "enum": ["arxiv", "brave", "duckduckgo", "exa", "tavily"], "description": "Search backend to use. Defaults to duckduckgo." },
                    "limit": { "type": "integer", "description": "Maximum number of results (default 5, max 10)" }
                },
                "required": ["query"]
            }),
            examples: vec![
                ToolExample {
                    description: "Search for latest news".to_string(),
                    input: serde_json::json!({"query": "latest Rust programming news 2026"}),
                    output: Some("Search results for 'latest Rust programming news 2026' (DuckDuckGo):\n\n1. Rust Blog\n   Official blog\n   URL: https://blog.rust-lang.org".to_string()),
                },
                ToolExample {
                    description: "Search ArXiv with limited results".to_string(),
                    input: serde_json::json!({"query": "machine learning", "backend": "arxiv", "limit": 3}),
                    output: None,
                },
            ],
            caveats: Vec::new(),
            side_effects: SideEffects::default(),
            preconditions: Vec::new(),
            related: Vec::new(),
        }
    }

    async fn execute(&self, arguments: &str) -> Result<String, ToolError> {
        let args: WebSearchArgs =
            serde_json::from_str(arguments).map_err(|e| ToolError::InvalidArguments {
                message: format!("Invalid arguments for websearch: {e}"),
            })?;

        let backend_name = args.backend.as_deref().unwrap_or("duckduckgo");
        let limit = args.limit.unwrap_or(5).min(10);
        let config = self.config.get();

        let provider: Box<dyn SearchProvider> = match backend_name {
            "arxiv" => Box::new(ArxivProvider::new()),
            "duckduckgo" => Box::new(DuckDuckGoProvider::new()),
            "tavily" => {
                let api_key = resolve_api_key(config, "tavily", "TAVILY_API_KEY")?;
                Box::new(
                    TavilyProvider::new(&api_key).map_err(|e| ToolError::ExecutionFailed {
                        message: format!("Tavily provider init failed: {e}"),
                    })?,
                )
            }
            "brave" => {
                let api_key = resolve_api_key(config, "brave", "BRAVE_API_KEY")?;
                Box::new(
                    BraveProvider::new(&api_key).map_err(|e| ToolError::ExecutionFailed {
                        message: format!("Brave provider init failed: {e}"),
                    })?,
                )
            }
            "exa" => {
                let api_key = resolve_api_key(config, "exa", "EXA_API_KEY")?;
                Box::new(
                    ExaProvider::new(&api_key).map_err(|e| ToolError::ExecutionFailed {
                        message: format!("Exa provider init failed: {e}"),
                    })?,
                )
            }
            _ => {
                return Err(ToolError::InvalidArguments {
                    message: format!("Unknown backend: {backend_name}"),
                });
            }
        };

        let results = web_search(SearchOptions {
            query: args.query.clone(),
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

        let mut output = format!(
            "Search results for '{}' ({}):\n\n",
            args.query, provider_label
        );
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
}
