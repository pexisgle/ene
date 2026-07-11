use ene_core::{BootstrapOptions, EneCoreError, EneHandle, MemoryConfig, bootstrap_runtime};

/// Initializes the actor via the shared bootstrap path.
pub async fn init() -> Result<EneHandle, EneCoreError> {
    tracing::info!("[Runtime] Initializing AI runtime...");

    let handle = EneHandle::new();

    let config = bootstrap_runtime(&handle, BootstrapOptions::from_disk()).await?;

    tracing::info!("[Runtime] AI runtime initialized successfully.");

    let mem_config = config.get_section::<MemoryConfig>().unwrap_or_default();
    if mem_config.enabled {
        tracing::info!("[Memory] Long-term memory enabled.");
        tracing::info!(
            "[Memory] DB: {}",
            mem_config
                .resolve_memory_db_path(&config.character)
                .display()
        );
    }

    Ok(handle)
}
