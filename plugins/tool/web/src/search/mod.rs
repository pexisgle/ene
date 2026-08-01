//! In-tree web search providers backed by workspace `reqwest`.

mod error;
mod providers;
mod types;

pub use error::SearchError;
pub(crate) use providers::search_client;
pub use providers::{ArxivProvider, DuckDuckGoProvider, ExaProvider, TavilyProvider};
pub use types::{SearchOptions, SearchProvider, SearchResult};

pub async fn web_search(options: SearchOptions) -> Result<Vec<SearchResult>, SearchError> {
    if options.query.is_empty() {
        return Err(SearchError::InvalidInput(
            "A search query is required".to_string(),
        ));
    }

    options.provider.search(&options).await
}
