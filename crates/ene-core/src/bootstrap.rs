//! Shared runtime bootstrap for desktop and CLI entry points.
//!
//! Phase 3 of the application startup sequence: configure the actor,
//! load the character card, and warm caches (tool embeddings, CCv3
//! character memories) before the first user turn.

use ene_cognition::{CognitionConfig, character::sync_character_memories};
use ene_config::EneConfig;

use crate::error::EneCoreError;
use crate::handle::EneHandle;

/// Options controlling how [`bootstrap_runtime`] obtains configuration.
pub struct BootstrapOptions {
    /// When `true`, load `settings.json` from disk via [`ene_config::load_config`].
    /// Desktop passes `false` and supplies a pre-loaded [`EneConfig`].
    pub load_from_disk: bool,
    /// Pre-loaded configuration (required when `load_from_disk` is `false`).
    pub config: Option<EneConfig>,
}

impl BootstrapOptions {
    /// Load configuration from the default on-disk paths (CLI).
    #[must_use]
    pub fn from_disk() -> Self {
        Self {
            load_from_disk: true,
            config: None,
        }
    }

    /// Use a configuration already loaded by the host (desktop).
    #[must_use]
    pub fn with_config(config: EneConfig) -> Self {
        Self {
            load_from_disk: false,
            config: Some(config),
        }
    }
}

/// Configure the actor, load the character card, and run startup warmups.
///
/// Schema files are written when loading from disk (via
/// [`ene_config::load_full_config`]). Desktop hosts should load config
/// once synchronously before calling this with [`BootstrapOptions::with_config`].
pub async fn bootstrap_runtime(
    handle: &EneHandle,
    opts: BootstrapOptions,
) -> Result<EneConfig, EneCoreError> {
    let config = if opts.load_from_disk {
        ene_config::load_config()?
    } else {
        opts.config.ok_or_else(|| {
            EneCoreError::Bootstrap("config required when load_from_disk is false".into())
        })?
    };

    handle.reconfigure(config.clone()).await?;

    if let Err(e) = handle.load_character(&config.character).await {
        tracing::warn!(component = "Bootstrap", error = %e, "Failed to load character card");
    }

    warmup_character_memories(handle).await;

    Ok(config)
}

/// Pre-sync CCv3 lorebook/style indices so the first conversation turn
/// does not pay the embedding cost.
async fn warmup_character_memories(handle: &EneHandle) {
    let snapshot = match handle.get_snapshot().await {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(
                component = "Bootstrap",
                error = %e,
                "Skipping character memory warmup: snapshot unavailable"
            );
            return;
        }
    };

    let cognition = snapshot
        .config
        .get_section::<CognitionConfig>()
        .unwrap_or_default();
    if !cognition.enabled {
        return;
    }

    let Some(card) = snapshot.character_card.as_ref() else {
        tracing::debug!(
            component = "Bootstrap",
            "Skipping character memory warmup: no character card loaded"
        );
        return;
    };

    let Some(store) = snapshot.memory.store() else {
        tracing::debug!(
            component = "Bootstrap",
            "Skipping character memory warmup: memory store unavailable"
        );
        return;
    };

    let Some(embedder) = snapshot.memory.embedder() else {
        tracing::debug!(
            component = "Bootstrap",
            "Skipping character memory warmup: embedder unavailable"
        );
        return;
    };

    match sync_character_memories(
        store,
        embedder,
        snapshot.card_name.as_str(),
        &snapshot.config.user_name,
        card,
        &cognition.character,
        None,
    )
    .await
    {
        Ok((report, hash)) => {
            if let Err(e) = handle.set_ccv3_memory_hash(hash).await {
                tracing::warn!(
                    component = "Bootstrap",
                    error = %e,
                    "Failed to persist CCv3 memory hash after warmup"
                );
            } else {
                tracing::info!(
                    component = "Bootstrap",
                    skipped = report.skipped,
                    lorebook_inserted = report.lorebook_inserted,
                    lorebook_updated = report.lorebook_updated,
                    style_inserted = report.style_inserted,
                    style_updated = report.style_updated,
                    archived = report.archived,
                    "Character memory warmup complete"
                );
            }
        }
        Err(e) => {
            tracing::warn!(
                component = "Bootstrap",
                error = %e,
                "Character memory warmup failed; first turn will retry"
            );
        }
    }
}
