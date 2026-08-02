//! Per-connection plugin context.

use crate::credentials::CredentialClient;
use std::sync::Arc;

/// Per-connection context handed to every provider method.
///
/// A plugin author receives `&PluginContext` as the first argument of each
/// `LlmPlugin` / `EmbedPlugin` / `TtsPlugin` / `SttPlugin` method. Today it
/// carries the credential client; settings access and call context are
/// intended to join it as the plugin surface grows.
///
/// The context is cheap to clone (the credential client is `Arc`-shared) so
/// dispatch code can move a copy into spawned tasks.
#[derive(Clone, Debug)]
pub struct PluginContext {
    credentials: Arc<CredentialClient>,
}

impl PluginContext {
    /// Builds a context wrapping `credentials`.
    #[must_use]
    pub fn new(credentials: CredentialClient) -> Self {
        Self {
            credentials: Arc::new(credentials),
        }
    }

    /// Returns the credential client for resolving host-held secrets.
    #[must_use]
    pub fn credentials(&self) -> &CredentialClient {
        &self.credentials
    }
}
