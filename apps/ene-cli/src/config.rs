use ene_core::{EneCoreError, EneHandle, MemoryConfig};

/// Initializes the actor, loading configuration and the active character card.
pub async fn init() -> Result<EneHandle, EneCoreError> {
    tracing::info!("[Runtime] Initializing AI runtime...");

    let handle = EneHandle::new();

    let config = handle.load_config().await?;

    if let Err(e) = handle.load_character(&config.character).await {
        tracing::warn!(error = %e, "[Runtime] Failed to load character card");
    }

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

    ene_config::write_schemas(&ene_config::paths::assets_dir());

    Ok(handle)
}
