mod arxiv;
mod duckduckgo;
mod exa;
mod tavily;

pub use arxiv::ArxivProvider;
pub use duckduckgo::DuckDuckGoProvider;
pub use exa::ExaProvider;
pub use tavily::TavilyProvider;

pub(super) fn extract_domain(url: &str) -> Option<String> {
    url::Url::parse(url)
        .ok()
        .and_then(|parsed| parsed.host_str().map(str::to_string))
}
