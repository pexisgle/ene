//! Read-only session query handle (#176, #271).
//!
//! See the [`crate::query`] module docs for why this bypasses the actor
//! mailbox entirely: session CRUD only ever touches `MemoryStore`, never the
//! actor's turn-execution state.

use crate::public_api::{PublicApiError, PublicExportedMessage, PublicSessionMeta};
use ene_store::MemoryStore;
use std::sync::Arc;

/// Read-only + mutating handle over stored sessions
/// (list / export / import / search / archive).
///
/// Obtained via [`crate::EneHandle::sessions`]. Cheap to clone (wraps a
/// single optional `Arc`).
#[derive(Clone)]
pub struct SessionQueryHandle {
    store: Option<Arc<MemoryStore>>,
}

impl SessionQueryHandle {
    pub(crate) const fn new(store: Option<Arc<MemoryStore>>) -> Self {
        Self { store }
    }

    fn store(&self) -> Result<&Arc<MemoryStore>, PublicApiError> {
        self.store.as_ref().ok_or_else(|| PublicApiError::Internal {
            message: "Memory store is not enabled".to_string(),
        })
    }

    /// List stored sessions, newest first (#176).
    ///
    /// Part of the API v1 contract (#269): errors are the stable
    /// [`PublicApiError`] categories, not `ene_store::EneMemoryError`.
    pub async fn list(
        &self,
        include_archived: bool,
        limit: usize,
    ) -> Result<Vec<PublicSessionMeta>, PublicApiError> {
        let metas = self.store()?.list_sessions(include_archived, limit).await?;
        Ok(metas.into_iter().map(Into::into).collect())
    }

    /// Export a session to a versioned, redacted JSON bundle (#176).
    ///
    /// Part of the API v1 contract (#269): errors are the stable
    /// [`PublicApiError`] categories, not `ene_store::EneMemoryError`.
    pub async fn export(&self, session_id: &str) -> Result<String, PublicApiError> {
        let export = self.store()?.build_export(session_id).await?;
        Ok(export.to_json()?)
    }

    /// Import a session from a JSON export bundle (#176).
    ///
    /// Part of the API v1 contract (#269): errors are the stable
    /// [`PublicApiError`] categories, not `ene_store::EneMemoryError`.
    pub async fn import(&self, json: &str) -> Result<i64, PublicApiError> {
        let export = ene_store::SessionExport::from_json(json)?;
        Ok(self.store()?.import_export(&export).await?)
    }

    /// Full-text search over stored conversation messages (#176).
    ///
    /// Part of the API v1 contract (#269): errors are the stable
    /// [`PublicApiError`] categories, and matches carry
    /// [`PublicExportedMessage`] rather than `ene_store::ExportedMessage`.
    pub async fn search(
        &self,
        query: &str,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<(String, PublicExportedMessage)>, PublicApiError> {
        let matches = self.store()?.search_messages(query, limit, offset).await?;
        Ok(matches
            .into_iter()
            .map(|(session_id, message)| (session_id, message.into()))
            .collect())
    }

    /// Archive or unarchive a session (#176).
    ///
    /// Part of the API v1 contract (#269): errors are the stable
    /// [`PublicApiError`] categories, not `ene_store::EneMemoryError`.
    pub async fn set_archived(
        &self,
        session_id: &str,
        archived: bool,
    ) -> Result<bool, PublicApiError> {
        Ok(self
            .store()?
            .set_session_archived(session_id, archived)
            .await?)
    }
}
