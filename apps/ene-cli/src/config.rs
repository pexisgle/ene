use crate::style;
use ene_core::{EneCoreError, EneHandle, MemoryConfig};

/// Initializes the actor, loading configuration and the active character card.
pub async fn init() -> Result<EneHandle, EneCoreError> {
    println!("{}", style::header("[Runtime] Initializing AI runtime..."));

    let handle = EneHandle::new();

    let config = handle.load_config().await?;

    if let Err(e) = handle.load_character(&config.character).await {
        eprintln!(
            "{}",
            style::error(format!(
                "[Runtime] Warning: Failed to load character card: {e}"
            ))
        );
    }

    println!(
        "{}",
        style::success("[Runtime] AI runtime initialized successfully.")
    );

    // Log long-term memory status if enabled
    let mem_config = config.get_section::<MemoryConfig>().unwrap_or_default();
    if mem_config.enabled {
        println!("{}", style::header("[Memory] Long-term memory enabled."));
        println!(
            "{}",
            style::header(format!(
                "[Memory] DB: {}",
                mem_config
                    .resolve_memory_db_path(&config.character)
                    .display()
            ))
        );
    }

    // Write configuration schemas after runtime initialization (which registers dynamic tool schemas).
    ene_config::write_schemas(&ene_config::paths::assets_dir());

    Ok(handle)
}
