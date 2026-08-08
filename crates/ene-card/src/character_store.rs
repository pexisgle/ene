use ene_config::config::atomic_write;
use ene_config::paths::character_settings_path;
use ene_config::{EneConfigError, HasConfigKey};
use parking_lot::RwLock;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::CharacterConfig;

/// Per-character settings store with dirty tracking for auto-save.
///
/// `CharacterConfigStore` is the persistence layer for
/// `character_settings.json`, separated from [`ene_config::ConfigStore`]
/// so the card subsystem does not leak into the settings core.
///
/// The intended usage is:
/// 1. Mutate the per-character config via
///    [`with_character_config_mut`](Self::with_character_config_mut) or
///    [`set_character_config`](Self::set_character_config).
/// 2. The store is automatically marked dirty on mutation.
/// 3. A periodic flush (e.g. a Bevy system each frame) calls
///    [`flush_if_dirty`](Self::flush_if_dirty).
pub struct CharacterConfigStore {
    character_config: RwLock<CharacterConfig>,
    character_dirty: AtomicBool,
}

impl std::fmt::Debug for CharacterConfigStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CharacterConfigStore")
            .field(
                "character_dirty",
                &self.character_dirty.load(Ordering::Acquire),
            )
            .finish_non_exhaustive()
    }
}

impl Default for CharacterConfigStore {
    fn default() -> Self {
        Self {
            character_config: RwLock::new(CharacterConfig::default()),
            character_dirty: AtomicBool::new(false),
        }
    }
}

impl CharacterConfigStore {
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
        let path = character_settings_path(character_name);
        let Ok(content) = std::fs::read_to_string(&path) else {
            *self.character_config.write() = CharacterConfig::default();
            return;
        };
        match serde_json::from_str::<CharacterConfig>(&content) {
            Ok(config) => *self.character_config.write() = config,
            Err(e) => {
                tracing::warn!(
                    component = "CharacterStore",
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
        T: serde::de::DeserializeOwned + Default + HasConfigKey,
    {
        self.character_config()
            .get_section::<T>()
            .unwrap_or_default()
    }

    /// Writes a typed section into the per-character config and marks dirty.
    pub fn set_character_section<T>(&self, section: &T)
    where
        T: serde::Serialize + HasConfigKey,
    {
        self.with_character_config_mut(|c| {
            if let Err(e) = c.set_section(section) {
                tracing::error!(component = "CharacterStore", error = %e, "Failed to set character config section");
            }
        });
    }

    /// Saves the per-character config to disk if it has been modified.
    ///
    /// Returns `Ok(true)` if any write occurred, `Ok(false)` if nothing was dirty.
    ///
    /// The dirty flag is only cleared **after** a successful write, preventing
    /// data loss if the write fails.
    pub fn flush_if_dirty(&self, character_name: Option<&str>) -> Result<bool, EneConfigError> {
        if self.character_dirty.load(Ordering::Acquire) {
            if let Some(name) = character_name {
                let char_config = self.character_config.read();
                let path = character_settings_path(name);
                let json = serde_json::to_string_pretty(&*char_config)?;
                atomic_write(&path, &json)?;
                drop(char_config);
            }
            self.character_dirty.store(false, Ordering::Release);
            return Ok(true);
        }
        Ok(false)
    }

    /// Forces a save of the per-character config regardless of dirty state.
    pub fn flush(&self, character_name: Option<&str>) -> Result<(), EneConfigError> {
        self.character_dirty.store(true, Ordering::Release);
        self.flush_if_dirty(character_name)?;
        Ok(())
    }

    /// Returns `true` if the per-character config has unsaved changes.
    pub fn is_dirty(&self) -> bool {
        self.character_dirty.load(Ordering::Acquire)
    }

    /// Marks the per-character config as dirty without modifying it.
    pub fn mark_dirty(&self) {
        self.character_dirty.store(true, Ordering::Release);
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
        let store = CharacterConfigStore::default();
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
        let store = CharacterConfigStore::default();

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
        let store = CharacterConfigStore::default();

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
