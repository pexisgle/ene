const fn default_string() -> String {
    String::new()
}

ene_config::define_config!(
    settings,
    "store",
    /// Configuration for the persistence-only SQLite-vec store.
    pub struct StoreConfig {
        /// Whether the store is enabled.
        pub enabled: bool = false,
        /// Database path.
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
