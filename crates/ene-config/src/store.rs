use parking_lot::RwLock;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::{CharacterConfig, EneConfig, save_full_config};

/// Centralized configuration store with dirty tracking for auto-save.
///
/// `ConfigStore` is the single persistence layer managed by `ene-config`.
/// It wraps [`EneConfig`] (global settings) and per-character [`CharacterConfig`],
/// tracking whether each has unsaved changes via atomic dirty flags.
///
/// The intended usage is:
/// 1. Mutate config via [`config_mut`](Self::config_mut) or [`set_config`](Self::set_config).
/// 2. The store is automatically marked dirty on mutation.
/// 3. A periodic flush (e.g. a Bevy system each frame) calls [`flush_if_dirty`](Self::flush_if_dirty).
pub struct ConfigStore {
    config: RwLock<EneConfig>,
    character_config: RwLock<CharacterConfig>,
    global_dirty: AtomicBool,
    character_dirty: AtomicBool,
}

impl std::fmt::Debug for ConfigStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConfigStore")
            .field("global_dirty", &self.global_dirty.load(Ordering::Acquire))
            .field(
                "character_dirty",
                &self.character_dirty.load(Ordering::Acquire),
            )
            .finish_non_exhaustive()
    }
}

impl ConfigStore {
    /// Creates a new store by loading the global config from disk.
    ///
    /// Uses the standard figment pipeline (defaults → `settings.json` → `ENE_` env vars).
    /// On any extract failure, falls back to `EneConfig::default()` and
    /// logs the error. This is the only call site that preserves the
    /// pre-#40 silent-default behavior, because the desktop / cli host
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
            character_config: RwLock::new(CharacterConfig::default()),
            global_dirty: AtomicBool::new(false),
            character_dirty: AtomicBool::new(false),
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
            character_config: RwLock::new(CharacterConfig::default()),
            global_dirty: AtomicBool::new(false),
            character_dirty: AtomicBool::new(false),
        })
    }

    /// Creates a store from an already-loaded [`EneConfig`].
    #[must_use]
    pub fn from_config(config: EneConfig) -> Self {
        Self {
            config: RwLock::new(config),
            character_config: RwLock::new(CharacterConfig::default()),
            global_dirty: AtomicBool::new(false),
            character_dirty: AtomicBool::new(false),
        }
    }

    // ── Global config access ──────────────────────────────────────────

    /// Returns a clone of the current global config.
    pub fn config(&self) -> EneConfig {
        self.config.read().clone()
    }

    /// Gives mutable access to the global config.
    /// Automatically marks the store as dirty after the closure runs.
    pub fn with_config_mut(&self, f: impl FnOnce(&mut EneConfig)) {
        f(&mut self.config.write());
        self.global_dirty.store(true, Ordering::Release);
    }

    /// Replaces the entire global config and marks dirty.
    pub fn set_config(&self, config: EneConfig) {
        *self.config.write() = config;
        self.global_dirty.store(true, Ordering::Release);
    }

    /// Reads a typed section from the global config.
    pub fn get_section<T>(&self) -> T
    where
        T: serde::de::DeserializeOwned + Default + crate::HasConfigKey,
    {
        self.config.read().get_section::<T>().unwrap_or_default()
    }

    /// Writes a typed section into the global config and marks dirty.
    pub fn set_section<T>(&self, section: &T)
    where
        T: serde::Serialize + crate::HasConfigKey,
    {
        let _ = self.config.write().set_section(section);
        self.global_dirty.store(true, Ordering::Release);
    }

    // ── Per-character config access ───────────────────────────────────

    /// Returns a clone of the current per-character config.
    pub fn character_config(&self) -> CharacterConfig {
        self.character_config.read().clone()
    }

    /// Gives mutable access to the per-character config.
    /// Automatically marks it as dirty.
    pub fn with_character_config_mut(&self, f: impl FnOnce(&mut CharacterConfig)) {
        f(&mut self.character_config.write());
        self.character_dirty.store(true, Ordering::Release);
    }

    /// Replaces the per-character config and loads its state from disk for the given character.
    pub fn load_character_config(&self, character_name: &str) {
        let path = crate::character_settings_path(character_name);
        let content = std::fs::read_to_string(&path).ok();
        let mut guard = self.character_config.write();
        *guard = content
            .and_then(|json| serde_json::from_str::<CharacterConfig>(&json).ok())
            .unwrap_or_default();
    }

    /// Replaces the per-character config and marks dirty.
    pub fn set_character_config(&self, config: CharacterConfig) {
        *self.character_config.write() = config;
        self.character_dirty.store(true, Ordering::Release);
    }

    /// `character_settings` の extra セクションを型安全に取得（新規）
    pub fn get_character_section<T>(&self) -> T
    where
        T: serde::de::DeserializeOwned + Default + crate::HasConfigKey,
    {
        self.character_config()
            .get_section::<T>()
            .unwrap_or_default()
    }

    /// `character_settings` の extra セクションを型安全に書き込み（新規）
    pub fn set_character_section<T>(&self, section: &T)
    where
        T: serde::Serialize + crate::HasConfigKey,
    {
        self.with_character_config_mut(|c| {
            let _ = c.set_section(section);
        });
    }

    // ── Persistence ───────────────────────────────────────────────────

    /// Saves the global config to disk if it has been modified.
    /// Saves the per-character config if dirty and `character_name` is given.
    ///
    /// Returns `Ok(true)` if any write occurred, `Ok(false)` if nothing was dirty.
    pub fn flush_if_dirty(&self, character_name: Option<&str>) -> std::io::Result<bool> {
        let global_saved = if self.global_dirty.swap(false, Ordering::AcqRel) {
            let config = self.config.read();
            save_full_config(&config)?;
            true
        } else {
            false
        };

        let char_saved = if self.character_dirty.swap(false, Ordering::AcqRel) {
            if let Some(name) = character_name {
                let char_config = self.character_config.read();
                let path = crate::character_settings_path(name);
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                let json = serde_json::to_string_pretty(&*char_config)
                    .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
                std::fs::write(&path, json)?;
                drop(char_config);
            }
            true
        } else {
            false
        };

        Ok(global_saved || char_saved)
    }

    /// Forces a save of both global and per-character config regardless of dirty state.
    pub fn flush(&self, character_name: Option<&str>) -> std::io::Result<()> {
        self.global_dirty.store(true, Ordering::Release);
        self.character_dirty.store(true, Ordering::Release);
        self.flush_if_dirty(character_name)?;
        Ok(())
    }

    /// Returns `true` if either config has unsaved changes.
    pub fn is_dirty(&self) -> bool {
        self.global_dirty.load(Ordering::Acquire) || self.character_dirty.load(Ordering::Acquire)
    }
}
