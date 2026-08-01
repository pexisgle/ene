use parking_lot::RwLock;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::config::atomic_write;
use crate::{CharacterConfig, EneConfig, EneConfigError, save_full_config};

/// Centralized configuration store with dirty tracking for auto-save.
///
/// `ConfigStore` is the single persistence layer managed by `ene-config`.
/// It wraps [`EneConfig`] (global settings) and per-character [`CharacterConfig`],
/// tracking whether each has unsaved changes via atomic dirty flags.
///
/// The intended usage is:
/// 1. Mutate config via [`with_config_mut`](Self::with_config_mut) or [`set_config`](Self::set_config).
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
    pub fn from_config(config: EneConfig) -> Self {
        Self {
            config: RwLock::new(config),
            character_config: RwLock::new(CharacterConfig::default()),
            global_dirty: AtomicBool::new(false),
            character_dirty: AtomicBool::new(false),
        }
    }

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
        if let Err(e) = self.config.write().set_section(section) {
            tracing::error!(component = "ConfigStore", error = %e, "Failed to set global config section");
        }
        self.global_dirty.store(true, Ordering::Release);
    }

    /// Returns a clone of the current per-character config.
    pub fn character_config(&self) -> CharacterConfig {
        self.character_config.read().clone()
    }

    /// Gives mutable access to the per-character config.
    /// Automatically marks it as dirty.
    ///
    /// **Note:** this always marks dirty regardless of whether the closure
    /// actually changed anything. If called in a hot loop, prefer
    /// [`set_character_config`](Self::set_character_config) which performs
    /// an equality check first.
    pub fn with_character_config_mut(&self, f: impl FnOnce(&mut CharacterConfig)) {
        f(&mut self.character_config.write());
        self.character_dirty.store(true, Ordering::Release);
    }

    /// Replaces the per-character config and loads its state from disk for the given character.
    pub fn load_character_config(&self, character_name: &str) {
        let path = crate::character_settings_path(character_name);
        let Ok(content) = std::fs::read_to_string(&path) else {
            *self.character_config.write() = CharacterConfig::default();
            return;
        };
        match serde_json::from_str::<CharacterConfig>(&content) {
            Ok(config) => *self.character_config.write() = config,
            Err(e) => {
                tracing::warn!(
                    component = "ConfigStore",
                    character = character_name,
                    path = %path.display(),
                    error = %e,
                    "Failed to parse character config, using defaults"
                );
                *self.character_config.write() = CharacterConfig::default();
            }
        }
    }

    /// Replaces the per-character config and marks dirty.
    ///
    /// If `config` is equal to the current value, the dirty flag is **not**
    /// set and the method returns early. This prevents spurious disk writes
    /// when called every frame with an unchanged state.
    pub fn set_character_config(&self, config: CharacterConfig) {
        let mut guard = self.character_config.write();
        if *guard == config {
            return;
        }
        *guard = config;
        self.character_dirty.store(true, Ordering::Release);
    }

    /// Reads a typed section from the per-character config.
    pub fn get_character_section<T>(&self) -> T
    where
        T: serde::de::DeserializeOwned + Default + crate::HasConfigKey,
    {
        self.character_config()
            .get_section::<T>()
            .unwrap_or_default()
    }

    /// Writes a typed section into the per-character config and marks dirty.
    pub fn set_character_section<T>(&self, section: &T)
    where
        T: serde::Serialize + crate::HasConfigKey,
    {
        self.with_character_config_mut(|c| {
            if let Err(e) = c.set_section(section) {
                tracing::error!(component = "ConfigStore", error = %e, "Failed to set character config section");
            }
        });
    }

    /// Saves the global config to disk if it has been modified.
    /// Saves the per-character config if dirty and `character_name` is given.
    ///
    /// Returns `Ok(true)` if any write occurred, `Ok(false)` if nothing was dirty.
    ///
    /// The dirty flag is only cleared **after** a successful write, preventing
    /// data loss if `save_full_config` fails.
    pub fn flush_if_dirty(&self, character_name: Option<&str>) -> Result<bool, EneConfigError> {
        let mut any_saved = false;

        if self.global_dirty.load(Ordering::Acquire) {
            let config = self.config.read();
            save_full_config(&config)?;
            drop(config);
            self.global_dirty.store(false, Ordering::Release);
            any_saved = true;
        }

        if self.character_dirty.load(Ordering::Acquire) {
            if let Some(name) = character_name {
                let char_config = self.character_config.read();
                let path = crate::character_settings_path(name);
                let json = serde_json::to_string_pretty(&*char_config)?;
                atomic_write(&path, &json)?;
                drop(char_config);
            }
            self.character_dirty.store(false, Ordering::Release);
            any_saved = true;
        }

        Ok(any_saved)
    }

    /// Forces a save of both global and per-character config regardless of dirty state.
    pub fn flush(&self, character_name: Option<&str>) -> Result<(), EneConfigError> {
        self.global_dirty.store(true, Ordering::Release);
        self.character_dirty.store(true, Ordering::Release);
        self.flush_if_dirty(character_name)?;
        Ok(())
    }

    /// Returns `true` if either config has unsaved changes.
    pub fn is_dirty(&self) -> bool {
        self.global_dirty.load(Ordering::Acquire) || self.character_dirty.load(Ordering::Acquire)
    }

    /// Marks the global config as dirty without modifying it.
    /// Use when the caller knows the in-memory state has diverged
    /// from disk and will call [`flush_if_dirty`] on the next cycle.
    pub fn mark_dirty(&self) {
        self.global_dirty.store(true, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pushing an unchanged [`CharacterConfig`] (the desktop does this every
    /// frame) must not flip the dirty flag, or the store would rewrite
    /// `character_settings.json` on every flush cycle.
    #[test]
    fn set_character_config_unchanged_value_does_not_mark_dirty() {
        let store = ConfigStore::from_config(EneConfig::default());
        assert!(!store.is_dirty(), "fresh store starts clean");

        let config = CharacterConfig::default();
        store.set_character_config(config.clone());
        assert!(
            !store.is_dirty(),
            "setting the same value must not mark the store dirty"
        );
    }

    /// A genuinely different [`CharacterConfig`] must still mark dirty so
    /// the change is persisted on the next flush.
    #[test]
    fn set_character_config_changed_value_marks_dirty() {
        let store = ConfigStore::from_config(EneConfig::default());

        let config = CharacterConfig {
            model_scale: 2.5,
            ..CharacterConfig::default()
        };
        store.set_character_config(config.clone());

        assert!(
            store.is_dirty(),
            "a changed value must mark the store dirty"
        );
        assert_eq!(store.character_config(), config);
    }

    /// The equality guard must consider the flattened `extra` map too, so a
    /// change that only touches a nested section is still persisted.
    #[test]
    fn set_character_config_extra_change_marks_dirty() {
        let store = ConfigStore::from_config(EneConfig::default());

        let config = CharacterConfig {
            extra: indexmap::IndexMap::from([(
                "motion".to_string(),
                serde_json::Value::String("wave".to_string()),
            )]),
            ..CharacterConfig::default()
        };
        store.set_character_config(config);

        assert!(
            store.is_dirty(),
            "an `extra`-only change must mark the store dirty"
        );
    }
}
