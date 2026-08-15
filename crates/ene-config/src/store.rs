use parking_lot::RwLock;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::{EneConfig, EneConfigError, save_full_config};

/// Centralized configuration store with dirty tracking for auto-save.
///
/// `ConfigStore` is the single persistence layer for global settings,
/// managed by `ene-config`. Per-character settings live in
/// `ene_card::CharacterConfigStore`; the two are kept separate so the
/// card subsystem does not leak into the settings core.
///
/// The intended usage is:
/// 1. Mutate config via [`with_config_mut`](Self::with_config_mut) or [`set_config`](Self::set_config).
/// 2. The store is automatically marked dirty on mutation.
/// 3. A periodic flush (e.g. a Bevy system each frame) calls [`flush_if_dirty`](Self::flush_if_dirty).
pub struct ConfigStore {
    config: RwLock<EneConfig>,
    global_dirty: AtomicBool,
}

impl std::fmt::Debug for ConfigStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConfigStore")
            .field("global_dirty", &self.global_dirty.load(Ordering::Acquire))
            .finish_non_exhaustive()
    }
}

impl ConfigStore {
    /// Creates a new store by loading the global config from disk.
    ///
    /// Uses the standard figment pipeline (defaults → `settings.json` → `ENE_` env vars).
    /// On any extract failure, falls back to `EneConfig::default()` and
    /// logs the error. This is the only call site that preserves the
    /// silent-default behavior, because the desktop / cli host
    /// must be able to construct a store before the user can fix the
    /// config file. Use [`Self::try_load`] to surface the error.
    pub fn load() -> Self {
        let config = match crate::load_config() {
            Ok(c) => c,
            Err(e) => {
                tracing::error!(component = "ConfigStore", error = %e, "Failed to load configuration, using defaults");
                EneConfig::default()
            }
        };
        Self {
            config: RwLock::new(config),
            global_dirty: AtomicBool::new(false),
        }
    }

    /// Like [`Self::load`] but propagates the load error. Use this when
    /// the caller wants the user to see the error directly (e.g. CLI
    /// startup, where failing fast is preferable to silently starting
    /// with an empty config).
    pub fn try_load() -> Result<Self, crate::EneConfigError> {
        let config = crate::load_config()?;
        Ok(Self {
            config: RwLock::new(config),
            global_dirty: AtomicBool::new(false),
        })
    }

    pub fn from_config(config: EneConfig) -> Self {
        Self {
            config: RwLock::new(config),
            global_dirty: AtomicBool::new(false),
        }
    }

    pub fn config(&self) -> EneConfig {
        self.config.read().clone()
    }

    pub fn with_config_mut(&self, f: impl FnOnce(&mut EneConfig)) {
        f(&mut self.config.write());
        self.global_dirty.store(true, Ordering::Release);
    }

    pub fn set_config(&self, config: EneConfig) {
        *self.config.write() = config;
        self.global_dirty.store(true, Ordering::Release);
    }

    pub fn get_section<T>(&self) -> T
    where
        T: serde::de::DeserializeOwned + Default + crate::HasConfigKey,
    {
        self.config.read().get_section::<T>().unwrap_or_default()
    }

    pub fn set_section<T>(&self, section: &T)
    where
        T: serde::Serialize + crate::HasConfigKey,
    {
        if let Err(e) = self.config.write().set_section(section) {
            tracing::error!(component = "ConfigStore", error = %e, "Failed to set global config section");
        }
        self.global_dirty.store(true, Ordering::Release);
    }

    /// Saves the global config to disk if it has been modified.
    ///
    /// Returns `Ok(true)` if any write occurred, `Ok(false)` if nothing was dirty.
    ///
    /// The dirty flag is only cleared **after** a successful write, preventing
    /// data loss if `save_full_config` fails.
    pub fn flush_if_dirty(&self) -> Result<bool, EneConfigError> {
        if self.global_dirty.load(Ordering::Acquire) {
            let config = self.config.read();
            save_full_config(&config)?;
            drop(config);
            self.global_dirty.store(false, Ordering::Release);
            return Ok(true);
        }
        Ok(false)
    }

    pub fn flush(&self) -> Result<(), EneConfigError> {
        self.global_dirty.store(true, Ordering::Release);
        self.flush_if_dirty()?;
        Ok(())
    }

    pub fn is_dirty(&self) -> bool {
        self.global_dirty.load(Ordering::Acquire)
    }

    /// Use when the caller knows the in-memory state has diverged
    /// from disk and will call [`flush_if_dirty`] on the next cycle.
    pub fn mark_dirty(&self) {
        self.global_dirty.store(true, Ordering::Release);
    }
}
