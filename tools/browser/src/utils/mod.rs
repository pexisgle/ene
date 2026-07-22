pub mod chrome;
pub mod session;

use std::sync::Arc;

/// Creates a default `BrowserSessionStore` wrapped in `Arc`.
///
/// Used by action structs' `#[serde(default)]` attribute so that
/// deserialized actions get a fresh (disconnected) store.
pub fn default_store() -> Arc<session::BrowserSessionStore> {
    Arc::new(session::BrowserSessionStore::new())
}
