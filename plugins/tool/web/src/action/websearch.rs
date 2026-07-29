use ene_plugin::prelude::*;
use std::fmt::Write;
use std::sync::{Arc, RwLock};

use crate::provider::WebSearchConfig;
use crate::search::{
    ArxivProvider, DuckDuckGoProvider, ExaProvider, SearchOptions, TavilyProvider, search_client,
    web_search,
};

fn default_config() -> Arc<RwLock<WebSearchConfig>> {
    Arc::new(RwLock::new(WebSearchConfig::default()))
}

/// A supported web-search backend.
///
/// Centralizes the backend name/label mapping so adding a provider
/// only requires touching this enum instead of several parallel
/// `match` blocks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Backend {
    Arxiv,
    DuckDuckGo,
    Exa,
    Tavily,
}

impl Backend {
    fn parse(name: &str) -> Option<Self> {
        match name {
            "arxiv" => Some(Self::Arxiv),
            "duckduckgo" => Some(Self::DuckDuckGo),
            "exa" => Some(Self::Exa),
            "tavily" => Some(Self::Tavily),
            _ => None,
        }
    }

    const fn name(self) -> &'static str {
        match self {
            Self::Arxiv => "arxiv",
            Self::DuckDuckGo => "duckduckgo",
            Self::Exa => "exa",
            Self::Tavily => "tavily",
        }
    }

    const fn label(self) -> &'static str {
        match self {
            Self::Arxiv => "ArXiv",
            Self::DuckDuckGo => "DuckDuckGo",
            Self::Exa => "Exa",
            Self::Tavily => "Tavily",
        }
    }
}

#[derive(Clone, Deserialize, JsonSchema, ToolAction)]
#[tool(
    namespace = "web",
    name = "search",
    summary = "Search the web for the latest information.",
    description = "Searches the web using the configured backend (duckduckgo, tavily, exa, or arxiv) and returns a list of relevant results with titles, URLs, and snippets.",
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
    #[arg(enum_values = "arxiv, duckduckgo, exa, tavily", default = "duckduckgo")]
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
        let backend = Backend::parse(backend_name).ok_or_else(|| ToolError::InvalidArguments {
            message: format!("Unknown backend: {backend_name}"),
        })?;
        let limit = self.limit.unwrap_or(5).min(10);
        // Snapshot the current config under a read lock so
        // a hot-reload from a reconfigure (which takes the
        // write lock) does not block the search. The
        // previous `OnceLock::get()` pattern only ever
        // returned the value set on the first call, so
        // updating an API key required a process restart.
        let config = match self.config.read() {
            Ok(guard) => Some(guard.clone()),
            Err(e) => {
                tracing::warn!("WebSearchConfig read lock poisoned: {e}");
                None
            }
        };

        // A single shared client is reused across providers so TLS
        // setup and connection pooling are not repeated per request.
        let client = search_client()
            .map_err(|e| ToolError::execution_failed(format!("HTTP client init failed: {e}")))?;

        let provider: Box<dyn crate::search::SearchProvider> = match backend {
            Backend::Arxiv => Box::new(ArxivProvider::new(client)),
            Backend::DuckDuckGo => Box::new(DuckDuckGoProvider::new(client)),
            Backend::Tavily => {
                let api_key = resolve_api_key(config.as_ref(), backend, "TAVILY_API_KEY")?;
                Box::new(TavilyProvider::new(&api_key, client).map_err(|e| {
                    ToolError::execution_failed(format!("Tavily provider init failed: {e}"))
                })?)
            }
            Backend::Exa => {
                let api_key = resolve_api_key(config.as_ref(), backend, "EXA_API_KEY")?;
                Box::new(ExaProvider::new(&api_key, client).map_err(|e| {
                    ToolError::execution_failed(format!("Exa provider init failed: {e}"))
                })?)
            }
        };

        let results = web_search(SearchOptions {
            query: self.query.clone(),
            max_results: Some(limit),
            provider,
        })
        .await
        .map_err(|e| ToolError::execution_failed(format!("Search failed: {e}")))?;

        if results.is_empty() {
            return Ok("No results found.".to_string());
        }

        let mut output = format!(
            "Search results for '{}' ({}):\n\n",
            self.query,
            backend.label()
        );
        for (i, result) in results.iter().enumerate() {
            let snippet = result.snippet.as_deref().unwrap_or("");
            // `fmt::Error` is `Copy`, so `drop()` would itself trip
            // `clippy::dropping_copy_types`; writing into a `String` via
            // `fmt::Write` never actually fails.
            #[expect(
                clippy::let_underscore_must_use,
                reason = "fmt::Write to a String is infallible in practice"
            )]
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
    backend: Backend,
    env_var: &str,
) -> Result<String, ToolError> {
    if let Some(cfg) = config {
        let configured = match backend {
            Backend::Tavily => &cfg.tavily_api_key,
            Backend::Exa => &cfg.exa_api_key,
            // Backends without an API key never call this helper.
            Backend::Arxiv | Backend::DuckDuckGo => "",
        };
        if !configured.is_empty() {
            return Ok(configured.to_string());
        }
    }
    std::env::var(env_var).map_err(|_| ToolError::execution_failed(format!("{env_var} not set. Set it in settings.json (tools.web.{}_api_key) or as environment variable {env_var}.", backend.name())))
}
