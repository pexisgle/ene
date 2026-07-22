use ene_config::ConfigStore;
use ene_runtime::{EneHandle, EneRuntimeError, StoreConfig, open_ready};
use std::path::Path;

/// Startup overrides sourced from CLI flags.
#[derive(Debug, Default)]
pub struct InitOptions {
    /// Optional explicit path to a `settings.json` file.
    pub config_path: Option<std::path::PathBuf>,
    /// Optional character card name or path override.
    pub character: Option<String>,
}

/// Initializes the actor via the ready-handle product path.
pub async fn init(opts: &InitOptions) -> Result<EneHandle, EneRuntimeError> {
    tracing::info!("[Runtime] Initializing AI runtime...");

    let mut config = match &opts.config_path {
        Some(path) => load_config_from_path(path)?,
        None => ConfigStore::try_load()?.config(),
    };
    if let Some(character) = &opts.character {
        config.character = character.clone();
    }

    let card = ene_config::load_character_card(&config.character)?;
    let handle = open_ready(config.clone(), card).await?;

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

    Ok(handle)
}

fn load_config_from_path(path: &Path) -> Result<ene_config::EneConfig, EneRuntimeError> {
    let assets_dir = path.parent().filter(|p| !p.as_os_str().is_empty());
    let config = match assets_dir {
        Some(dir) => ene_config::load_config_from(dir, path)?,
        None => ene_config::load_config_from(Path::new("."), path)?,
    };
    Ok(config)
}
