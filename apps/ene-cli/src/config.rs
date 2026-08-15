use ene_config::ConfigStore;
use ene_runtime::{EneHandle, EneRuntimeError, StoreConfig, open_ready};
use std::path::Path;

#[derive(Debug, Default)]
pub struct InitOptions {
    pub config_path: Option<std::path::PathBuf>,
    pub character: Option<String>,
}

pub async fn init(opts: &InitOptions) -> Result<EneHandle, EneRuntimeError> {
    tracing::info!("[Runtime] Initializing AI runtime...");

    // Write JSON schemas once at startup rather than on every config load.
    ene_config::write_schemas(ene_config::assets_dir());
    ene_card::write_character_schemas(ene_config::assets_dir());

    let mut config = match &opts.config_path {
        Some(path) => load_config_from_path(path)?,
        None => ConfigStore::try_load()?.config(),
    };
    if let Some(character) = &opts.character {
        config.character = character.clone();
    }

    let card = ene_card::load_character_card_localized(
        &config.character,
        &crate::i18n::active_language_code(),
    )?;
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
    // Schemas are written to the global assets dir in `init` above; the
    // `--config-path` override only affects where `settings.json` is read
    // from, so no per-path assets directory is threaded through here.
    let config = ene_config::load_config_from(path)?;
    Ok(config)
}
