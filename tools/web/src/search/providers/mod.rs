mod arxiv;
mod brave;
mod duckduckgo;
mod exa;
mod tavily;

pub use arxiv::ArxivProvider;
pub use brave::BraveProvider;
pub use duckduckgo::DuckDuckGoProvider;
pub use exa::ExaProvider;
pub use tavily::TavilyProvider;

/// Extracts the host (domain) component from a URL string.
pub(super) fn extract_domain(url: &str) -> Option<String> {
    url::Url::parse(url)
        .ok()
        .and_then(|parsed| parsed.host_str().map(str::to_string))
}
