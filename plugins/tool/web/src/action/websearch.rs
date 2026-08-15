use ene_plugin::prelude::*;
use std::fmt::Write;
use std::sync::{Arc, RwLock};

use crate::broker::WebBroker;
use crate::provider::WebSearchConfig;
use crate::search::{
    ArxivProvider, DuckDuckGoProvider, ExaProvider, SearchOptions, TavilyProvider, web_search,
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
    #[tool(skip)]
    #[serde(skip)]
    broker: Arc<WebBroker>,
    query: String,
    #[arg(enum_values = "arxiv, duckduckgo, exa, tavily", default = "duckduckgo")]
    #[serde(default)]
    backend: Option<String>,
    #[arg(minimum = 1, maximum = 10, default = "5")]
    #[serde(default)]
    limit: Option<u32>,
}

impl WebSearchAction {
    pub const fn new(config: Arc<RwLock<WebSearchConfig>>, broker: Arc<WebBroker>) -> Self {
        Self {
            config,
            broker,
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
        // write lock) does not block the search. A
        // `OnceLock::get()` would only return the value set
        // on the first call, so changing the credential name
        // would require a process restart.
        let config = match self.config.read() {
            Ok(guard) => Some(guard.clone()),
            Err(e) => {
                tracing::warn!("WebSearchConfig read lock poisoned: {e}");
                None
            }
        };

        let provider: Box<dyn crate::search::SearchProvider> = match backend {
            Backend::Arxiv => Box::new(ArxivProvider::new(Arc::clone(&self.broker))),
            Backend::DuckDuckGo => Box::new(DuckDuckGoProvider::new(Arc::clone(&self.broker))),
            Backend::Tavily => {
                let credential =
                    resolve_credential_name(config.as_ref(), backend, "tavily_api_key");
                Box::new(
                    TavilyProvider::new(&credential, Arc::clone(&self.broker)).map_err(|e| {
                        ToolError::execution_failed(format!("Tavily provider init failed: {e}"))
                    })?,
                )
            }
            Backend::Exa => {
                let credential = resolve_credential_name(config.as_ref(), backend, "exa_api_key");
                Box::new(
                    ExaProvider::new(&credential, Arc::clone(&self.broker)).map_err(|e| {
                        ToolError::execution_failed(format!("Exa provider init failed: {e}"))
                    })?,
                )
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

fn resolve_credential_name(
    config: Option<&WebSearchConfig>,
    backend: Backend,
    default: &str,
) -> String {
    let credential = config.map_or(default, |cfg| match backend {
        Backend::Tavily => cfg.tavily_credential.as_str(),
        Backend::Exa => cfg.exa_credential.as_str(),
        Backend::Arxiv | Backend::DuckDuckGo => default,
    });
    if credential.trim().is_empty() {
        default.to_string()
    } else {
        credential.trim().to_string()
    }
}
