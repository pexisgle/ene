//! Host-side helpers for loading config + card before [`crate::EneHandle::open`].
//!
//! Product path: apps load config via [`ene_config::ConfigStore`] and the
//! character card via [`ene_config::load_character_card`], then call
//! [`crate::EneHandle::open`]. This module no longer drives multi-step
//! `new` / `reconfigure` / `load_character` on an unready handle.

use ene_config::{CharacterCardV3, ConfigStore, EneConfig, load_character_card};

use crate::error::EneRuntimeError;
use crate::handle::EneHandle;

/// Load config (fail-hard) + character card, then open a ready handle.
///
/// Intended for CLI startup.
pub async fn open_from_disk() -> Result<(EneHandle, EneConfig), EneRuntimeError> {
    let store = ConfigStore::try_load()?;
    let config = store.config();
    let card = load_character_card(&config.character)?;
    let handle = EneHandle::open(config.clone(), card).await?;
    Ok((handle, config))
}

/// Open a ready handle from an already-loaded config (desktop).
pub async fn open_with_config(config: EneConfig) -> Result<EneHandle, EneRuntimeError> {
    let card = load_character_card(&config.character)?;
    EneHandle::open(config, card).await
}

/// Open a ready handle from config + card values (no file I/O).
pub async fn open_ready(
    config: EneConfig,
    card: CharacterCardV3,
) -> Result<EneHandle, EneRuntimeError> {
    EneHandle::open(config, card).await
}
