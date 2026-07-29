const fn default_string() -> String {
    String::new()
}

const fn default_max_backups() -> usize {
    5
}

ene_config::define_config!(
    settings,
    "store",
    /// Configuration for the persistence-only SQLite-vec store.
    pub struct StoreConfig {
        /// Whether the store is enabled.
        pub enabled: bool = false,
        /// Create a file backup before applying pending migrations (#239).
        pub backup_on_migrate: bool = true,
        /// Maximum number of `{db}.bak.*` backups to retain (#239).
        pub max_backups: usize = default_max_backups(),
        /// Run `PRAGMA integrity_check` when opening the database (#239).
        pub integrity_check_on_open: bool = false,
        /// Database path.
        #[serde(skip_deserializing, default, skip_serializing)]
        #[schemars(skip)]
        pub db_path: String = default_string(),
    }
);

impl StoreConfig {
    /// Resolves the effective database path, defaulting to a file inside the
    /// character's directory (`assets/characters/{name}/memory.db`).
    pub fn resolve_memory_db_path(&self, character_name: &str) -> std::path::PathBuf {
        if !self.db_path.trim().is_empty() {
            return std::path::PathBuf::from(&self.db_path);
        }
        ene_config::paths::character_dir(character_name).join("memory.db")
    }
}
