use ene_config::ConfigStore;
use ene_runtime::{EneHandle, EneRuntimeError, StoreConfig, open_from_disk};

/// Initializes the actor via the ready-handle product path.
pub async fn init() -> Result<EneHandle, EneRuntimeError> {
    tracing::info!("[Runtime] Initializing AI runtime...");

    let (handle, config) = open_from_disk().await?;

    tracing::info!("[Runtime] AI runtime initialized successfully.");

    let mem_config = config.get_section::<StoreConfig>().unwrap_or_default();
    if mem_config.enabled {
        tracing::info!("[Memory] Long-term memory enabled.");
        tracing::info!(
            "[Memory] DB: {}",
            mem_config
                .resolve_memory_db_path(&config.character)
                .display()
        );
    }

    // Keep ConfigStore load path documented for hosts that need dirty tracking.
    let _ = ConfigStore::from_config(config);

    Ok(handle)
}
