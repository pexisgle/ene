use crate::style;
use ene_core::{EneCoreError, EneRuntime, MemoryConfig};

/// Initializes the core AI runtime, loading configuration and the active character card.
pub async fn init() -> Result<EneRuntime, EneCoreError> {
    println!("{}", style::header("[Runtime] Initializing AI runtime..."));

    let mut runtime = EneRuntime::init().await?;

    runtime.config().load().await?;
    runtime.character().load()?;

    println!(
        "{}",
        style::success("[Runtime] AI runtime initialized successfully.")
    );

    // Log long-term memory status if enabled in the loaded configuration
    let mem_config = runtime
        .config
        .get_section::<MemoryConfig>()
        .unwrap_or_default();
    if mem_config.enabled {
        println!("{}", style::header("[Memory] Long-term memory enabled."));
        println!(
            "{}",
            style::header(format!(
                "[Memory] DB: {}",
                mem_config.resolve_memory_db_path().display()
            ))
        );
    }

    Ok(runtime)
}
