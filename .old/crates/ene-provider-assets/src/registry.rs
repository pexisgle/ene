use std::sync::Arc;

use parking_lot::Mutex;

use crate::catalog_fetch::{
    CachedCatalog, cache_stale, fetch_runtime_catalog, load_cached_catalog, save_cached_catalog,
};
use crate::error::AssetError;
use crate::runtime_catalog::RuntimeCatalog;

pub const HOST_CATALOG_PLUGINS: &[&str] = &["provider.gguf", "provider.voicevox"];

#[must_use]
pub fn host_catalog_plugin(plugin_id: &str) -> bool {
    HOST_CATALOG_PLUGINS.contains(&plugin_id)
}

/// Cached GitHub release catalogs for host-managed provider assets.
pub struct CatalogRegistry {
    client_cache: Mutex<std::collections::HashMap<String, CachedCatalog>>,
}

impl CatalogRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self {
            client_cache: Mutex::new(std::collections::HashMap::new()),
        }
    }

    /// Load from memory, disk cache, or GitHub (when stale / missing).
    pub async fn ensure_fresh(
        &self,
        plugin_id: &str,
        force: bool,
    ) -> Result<RuntimeCatalog, AssetError> {
        if !host_catalog_plugin(plugin_id) {
            return Ok(RuntimeCatalog {
                plugin_id: plugin_id.to_owned(),
                assets: Vec::new(),
                fetched_at: None,
                error: None,
            });
        }
        if !force {
            if let Some(cached) = self.client_cache.lock().get(plugin_id).cloned()
                && !cache_stale(&cached)
            {
                return Ok(cached.catalog);
            }
            if let Some(cached) = load_cached_catalog(plugin_id)
                && !cache_stale(&cached)
            {
                self.client_cache
                    .lock()
                    .insert(plugin_id.to_owned(), cached.clone());
                return Ok(cached.catalog);
            }
        }
        self.refresh(plugin_id).await
    }

    /// Force-fetch from GitHub and update caches.
    pub async fn refresh(&self, plugin_id: &str) -> Result<RuntimeCatalog, AssetError> {
        match fetch_runtime_catalog(plugin_id).await {
            Ok(catalog) => {
                save_cached_catalog(plugin_id, &catalog)?;
                let cached = crate::catalog_fetch::wrap_cached_catalog(catalog.clone());
                self.client_cache
                    .lock()
                    .insert(plugin_id.to_owned(), cached);
                Ok(catalog)
            }
            Err(err) => {
                if let Some(cached) = load_cached_catalog(plugin_id) {
                    return Ok(cached.catalog);
                }
                Err(err)
            }
        }
    }
}

impl Default for CatalogRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[must_use]
pub fn shared_registry() -> Arc<CatalogRegistry> {
    static REGISTRY: std::sync::OnceLock<Arc<CatalogRegistry>> = std::sync::OnceLock::new();
    Arc::clone(REGISTRY.get_or_init(|| Arc::new(CatalogRegistry::new())))
}
