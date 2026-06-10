fn default_string() -> String {
    String::new()
}

ene_config::define_config!(
    settings,
    "memory",
    /// Configuration for the memory (SQLite-vec) subsystem.
    pub struct MemoryConfig {
        /// Whether memory is enabled.
        pub enabled: bool = false,
        /// Database path.
        pub db_path: String = default_string(),
        /// Recall limit.
        pub recall_limit: usize = 5,
        /// Similarity threshold.
        pub similarity_threshold: f32 = 0.5,
        /// Time decay hours.
        pub time_decay_hours: f64 = 24.0,
        /// Similarity weight.
        pub similarity_weight: f64 = 0.7,
        /// Recency weight.
        pub recency_weight: f64 = 0.3,
    }
);

impl MemoryConfig {
    /// Resolves the effective database path, defaulting to a file inside the
    /// character's directory (`assets/characters/{name}/memory.db`).
    #[must_use]
    pub fn resolve_memory_db_path(&self, character_name: &str) -> std::path::PathBuf {
        if !self.db_path.trim().is_empty() {
            return std::path::PathBuf::from(&self.db_path);
        }
        ene_config::paths::character_dir(character_name).join("memory.db")
    }
}
