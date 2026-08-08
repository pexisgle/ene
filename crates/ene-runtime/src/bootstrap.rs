//! Host-side helpers for loading config + card before [`crate::EneHandle::open`].
//!
//! Product path: apps load config via [`ene_config::ConfigStore`] and the
//! character card via [`ene_card::load_character_card_localized`] (the
//! locale comes from `mind.language`, falling back to the system locale),
//! then call [`crate::EneHandle::open`]. This module does not drive
//! multi-step `new` / `reconfigure` / `load_character` on an unready handle.

use ene_card::{CharacterCardV3, load_character_card_localized};
use ene_config::{ConfigStore, EneConfig};
use ene_mind::MindConfig;

use crate::error::EneRuntimeError;
use crate::handle::EneHandle;

/// Load config (fail-hard) + character card, then open a ready handle.
///
/// Intended for CLI startup.
pub async fn open_from_disk() -> Result<(EneHandle, EneConfig), EneRuntimeError> {
    // Write JSON schemas once at startup rather than on every config load.
    ene_config::write_schemas(ene_config::assets_dir());
    ene_card::write_character_schemas(ene_config::assets_dir());

    let store = ConfigStore::try_load()?;
    let config = store.config();
    let card = load_character_card_localized(&config.character, &card_language(&config))?;
    let handle = EneHandle::open(config.clone(), card).await?;
    Ok((handle, config))
}

/// Open a ready handle from an already-loaded config (desktop).
pub async fn open_with_config(config: EneConfig) -> Result<EneHandle, EneRuntimeError> {
    // Write JSON schemas once at startup rather than on every config load.
    ene_config::write_schemas(ene_config::assets_dir());
    ene_card::write_character_schemas(ene_config::assets_dir());

    let card = load_character_card_localized(&config.character, &card_language(&config))?;
    EneHandle::open(config, card).await
}

/// Open a ready handle from config + card values (no file I/O).
pub async fn open_ready(
    config: EneConfig,
    card: CharacterCardV3,
) -> Result<EneHandle, EneRuntimeError> {
    EneHandle::open(config, card).await
}

/// App language driving card localization: `mind.language` (which the
/// desktop keeps synced with its UI language), with the system locale as the
/// fallback when unset.
fn card_language(config: &EneConfig) -> String {
    config.get_section::<MindConfig>().map_or_else(
        |_| ene_config::system_language().to_string(),
        |mind| mind.resolved_language().to_string(),
    )
}
