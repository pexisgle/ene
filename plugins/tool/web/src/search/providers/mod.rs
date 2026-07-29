mod arxiv;
mod duckduckgo;
mod exa;
mod tavily;

pub use arxiv::ArxivProvider;
pub use duckduckgo::DuckDuckGoProvider;
pub use exa::ExaProvider;
pub use tavily::TavilyProvider;

use std::time::Duration;

use crate::search::error::SearchError;

const USER_AGENT: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/125.0.0.0 Safari/537.36";

/// Builds a `reqwest::Client` shared across search providers so that
/// TLS setup, DNS resolution, and connection pooling are reused across
/// requests instead of being re-created on every `search()` call.
pub(crate) fn search_client() -> Result<reqwest::Client, SearchError> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .user_agent(USER_AGENT)
        .build()
        .map_err(|e| SearchError::ConfigError(format!("HTTP client init failed: {e}")))
}

/// Extracts the host (domain) component from a URL string.
pub(super) fn extract_domain(url: &str) -> Option<String> {
    url::Url::parse(url)
        .ok()
        .and_then(|parsed| parsed.host_str().map(str::to_string))
}
