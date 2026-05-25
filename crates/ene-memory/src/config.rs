#![allow(missing_docs)]

fn default_string() -> String {
    String::new()
}

ene_config::define_config!(
    "memory",
    /// Configuration for the memory (SQLite-vec) subsystem.
    pub struct MemoryConfig {
        pub enabled: bool = false,
        pub db_path: String = default_string(),
        pub recall_limit: usize = 5,
        pub similarity_threshold: f32 = 0.5,
        pub time_decay_hours: f64 = 24.0,
        pub similarity_weight: f64 = 0.7,
        pub recency_weight: f64 = 0.3,

        // ── Tool RAG ─────────────────────────────────────────────────────────────
        /// Tool RAG設定 — ユーザー入力に関連するツールのみを動的に選択してtoken消費を抑制
        pub tool_rag_enabled: bool = true,
        pub tool_rag_limit: usize = 6,
        /// 常に含めるツール名（RAG絞り込み後も必ず残す）
        pub tool_rag_always_include: Vec<String> = vec![
            "question".to_string(),
            "todo".to_string(),
            "get_current_time".to_string(),
        ],

        // ── Summarization Model ──────────────────────────────────────────────────
        /// 要約生成に使うモデル（空の場合はチャット用モデルを使用）
        pub summarization_model: String = default_string(),
        /// 要約生成に使う base_url（空の場合はチャット用 base_url を使用）
        pub summarization_base_url: String = default_string(),
    }
);

impl MemoryConfig {
    /// Resolves the effective database path, defaulting to a file next to the character card.
    pub fn resolve_memory_db_path(&self) -> std::path::PathBuf {
        if !self.db_path.trim().is_empty() {
            return std::path::PathBuf::from(&self.db_path);
        }
        let settings = ene_config::get_global_settings();
        let card_path = std::path::Path::new(&settings.character);
        let dir = card_path.parent().unwrap_or(std::path::Path::new("."));
        dir.join("memory.db")
    }

    /// Resolves the effective summarisation model, falling back to the chat model.
    pub fn resolve_summarization_model(&self) -> String {
        if !self.summarization_model.trim().is_empty() {
            return self.summarization_model.clone();
        }
        let settings = ene_config::get_global_settings();
        if let Some(model) = settings.extra.get("provider")
            .and_then(|p| p.get("model"))
            .and_then(|v| v.as_str())
        {
            if !model.trim().is_empty() {
                return model.to_string();
            }
        }
        "gpt-4o-mini".to_string()
    }

    /// Resolves the effective summarisation base URL, falling back to the provider settings.
    pub fn resolve_summarization_base_url(&self) -> Result<String, ene_config::ConfigError> {
        if !self.summarization_base_url.trim().is_empty() {
            return Ok(self.summarization_base_url.clone());
        }
        let settings = ene_config::get_global_settings();
        if let Some(base_url) = settings.extra.get("provider")
            .and_then(|p| p.get("base_url"))
            .and_then(|v| v.as_str())
        {
            if !base_url.trim().is_empty() {
                return Ok(base_url.to_string());
            }
        }
        Err(ene_config::ConfigError::MissingBaseUrl {
            env_var: String::new(),
        })
    }
}
