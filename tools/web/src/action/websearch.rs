use ene_tool_common::prelude::*;
use std::fmt::Write;
use std::sync::{Arc, RwLock};

use crate::provider::WebSearchConfig;
use websearch::providers::{
    ArxivProvider, BraveProvider, DuckDuckGoProvider, ExaProvider, TavilyProvider,
};
use websearch::{SearchOptions, SearchProvider, web_search};

fn default_config() -> Arc<RwLock<WebSearchConfig>> {
    Arc::new(RwLock::new(WebSearchConfig::default()))
}

#[derive(Clone, Deserialize, JsonSchema, ToolAction)]
#[tool(
    namespace = "web",
    name = "search",
    summary = "Search the web for the latest information.",
    description = "Searches the web using the configured backend (duckduckgo, tavily, brave, exa, or arxiv) and returns a list of relevant results with titles, URLs, and snippets.",
    category = "WebSearch",
    keywords_primary = "search, web, google, internet, lookup",
    side_effects = "Network { external: true }"
)]
pub struct WebSearchAction {
    #[tool(skip)]
    #[serde(skip, default = "default_config")]
    config: Arc<RwLock<WebSearchConfig>>,
    /// The search query.
    query: String,
    /// Search backend to use. Defaults to duckduckgo.
    #[arg(
        enum_values = "arxiv, brave, duckduckgo, exa, tavily",
        default = "duckduckgo"
    )]
    #[serde(default)]
    backend: Option<String>,
    /// Maximum number of results (default 5, max 10).
    #[arg(minimum = 1, maximum = 10, default = "5")]
    #[serde(default)]
    limit: Option<u32>,
}

impl WebSearchAction {
    pub const fn new(config: Arc<RwLock<WebSearchConfig>>) -> Self {
        Self {
            config,
            query: String::new(),
            backend: None,
            limit: None,
        }
    }

    async fn run(&self) -> Result<String, ToolError> {
        let backend_name = self.backend.as_deref().unwrap_or("duckduckgo");
        let limit = self.limit.unwrap_or(5).min(10);
        // Snapshot the current config under a read lock so
        // a hot-reload from a reconfigure (which takes the
        // write lock) does not block the search. The
        // previous `OnceLock::get()` pattern only ever
        // returned the value set on the first call, so
        // updating an API key required a process restart.
        let config = self.config.read().ok().map(|g| g.clone());

        let provider: Box<dyn SearchProvider> = match backend_name {
            "arxiv" => Box::new(ArxivProvider::new()),
            "duckduckgo" => Box::new(DuckDuckGoProvider::new()),
            "tavily" => {
                let api_key = resolve_api_key(config.as_ref(), "tavily", "TAVILY_API_KEY")?;
                Box::new(
                    TavilyProvider::new(&api_key).map_err(|e| ToolError::ExecutionFailed {
                        message: format!("Tavily provider init failed: {e}"),
                    })?,
                )
            }
            "brave" => {
                let api_key = resolve_api_key(config.as_ref(), "brave", "BRAVE_API_KEY")?;
                Box::new(
                    BraveProvider::new(&api_key).map_err(|e| ToolError::ExecutionFailed {
                        message: format!("Brave provider init failed: {e}"),
                    })?,
                )
            }
            "exa" => {
                let api_key = resolve_api_key(config.as_ref(), "exa", "EXA_API_KEY")?;
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
            query: self.query.clone(),
            max_results: Some(limit),
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
            self.query, provider_label
        );
        for (i, result) in results.iter().enumerate() {
            let snippet = result.snippet.as_deref().unwrap_or("");
            let _ = write!(
                output,
                "{}. {}\n   {snippet}\n   URL: {}\n\n",
                i + 1,
                result.title,
                result.url,
            );
        }
        Ok(output)
    }
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
