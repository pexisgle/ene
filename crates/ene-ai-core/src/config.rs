use crate::error::AiCoreError;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "snake_case")]
pub struct AiProviderSettings {
    pub provider_name: String,
    pub model: String,
    pub base_url: String,
    pub api_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "snake_case")]
pub struct AiEmbeddingSettings {
    pub provider_type: EmbeddingProviderType,
    pub model: String,
    pub base_url: String,
    pub dimensions: Option<usize>,
    pub gguf_quantization: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "snake_case")]
pub struct AiSettings {
    pub provider: AiProviderSettings,
    pub character_card_path: String,
    pub user_name: String,
    pub runtime_rules: String,
    pub tool_calling_enabled: bool,
    pub max_tool_call_rounds: usize,
    pub mcp_servers: Vec<McpServerConfig>,

    pub embedding: AiEmbeddingSettings,
    pub memory: AiMemorySettings,

    pub sandbox: AiSandboxSettings,

    pub tools: AiToolSettings,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum EmbeddingProviderType {
    Api,
    #[default]
    Local,
}

impl EmbeddingProviderType {
    pub fn label(&self) -> &'static str {
        match self {
            EmbeddingProviderType::Api => "API (OpenAI-compatible)",
            EmbeddingProviderType::Local => "Local (GGUF / Candle)",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "snake_case")]
pub struct AiMemorySettings {
    pub enabled: bool,
    pub db_path: String,
    pub recall_limit: usize,
    pub similarity_threshold: f32,
    pub time_decay_hours: f64,
    pub similarity_weight: f64,
    pub recency_weight: f64,

    // ── Session Auto-Split ───────────────────────────────────────────────────
    /// セッション自動分割を有効にするか
    pub auto_session_split: bool,
    /// 時間ベースの分割閾値（分）— この時間以上発言がなければ自動分割
    pub session_timeout_minutes: u64,
    /// トピック変更の embedding 類似度閾値 (0.0–1.0)
    /// 前回入力との類似度がこの値を下回ったらトピック変更と判定
    pub topic_change_threshold: f32,
    /// 分割前の最小ターン数（短すぎる会話は要約しない）
    pub min_turns_before_split: usize,
    /// 要約をプロンプトに注入する最大数
    pub summary_recall_limit: usize,

    // ── Tool RAG ─────────────────────────────────────────────────────────────
    /// Tool RAG設定 — ユーザー入力に関連するツールのみを動的に選択してtoken消費を抑制
    pub tool_rag_enabled: bool,
    pub tool_rag_limit: usize,
    /// 常に含めるツール名（RAG絞り込み後も必ず残す）
    pub tool_rag_always_include: Vec<String>,

    // ── Summarization Model ──────────────────────────────────────────────────
    /// 要約生成に使うモデル（空の場合はチャット用モデルを使用）
    pub summarization_model: String,
    /// 要約生成に使う base_url（空の場合はチャット用 base_url を使用）
    pub summarization_base_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "snake_case")]
pub struct AiSandboxSettings {
    pub enabled: bool,
    pub allowed_directories: Vec<String>,
    pub writable_directories: Vec<String>,
    pub blocked_commands: Vec<String>,
    pub max_read_bytes: usize,
    pub max_write_bytes: usize,
    pub shell_timeout_ms: u64,
    pub max_shell_output_bytes: usize,
    pub max_shell_output_lines: usize,
}

/// ツールごとのエントリ — enable フラグ + ツール固有設定
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolEntry {
    /// 有効/無効
    pub enable: bool,
    /// ツール固有の設定 (enable 以外の全フィールド)
    #[serde(flatten)]
    pub config: serde_json::Value,
}

/// ツール設定 — ツール名 → ToolEntry のマップ
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AiToolSettings {
    /// ツールごとのエントリ
    ///
    /// 例:
    /// ```json
    /// { "fs": { "enable": true },
    ///   "web": { "enable": true, "search_backends": { "tavily": { "api_key": "..." } } }
    /// }
    /// ```
    /// enable=true のツールだけが起動され、余分なフィールドはそのツールの設定として渡される。
    pub tools: HashMap<String, ToolEntry>,
}

impl Default for AiToolSettings {
    fn default() -> Self {
        let mut tools = HashMap::new();
        for name in ["fs", "web", "browser", "utility", "app"] {
            tools.insert(
                name.to_string(),
                ToolEntry {
                    enable: true,
                    config: serde_json::Value::Object(Default::default()),
                },
            );
        }
        Self { tools }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerConfig {
    pub name: String,
    pub enabled: bool,
    pub transport: McpTransport,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum McpTransport {
    Stdio { command: String, args: Vec<String> },
    Http { url: String },
}

impl Default for AiEmbeddingSettings {
    fn default() -> Self {
        Self {
            provider_type: EmbeddingProviderType::Local,
            model: "jina-embeddings-v5-text-small".to_string(),
            base_url: String::new(),
            dimensions: None,
            gguf_quantization: "F16".to_string(),
        }
    }
}

impl Default for AiProviderSettings {
    fn default() -> Self {
        Self {
            provider_name: "openai-compatible".to_string(),
            model: "gpt-4o-mini".to_string(),
            base_url: String::new(),
            api_key: String::new(),
        }
    }
}

impl Default for AiMemorySettings {
    fn default() -> Self {
        Self {
            enabled: false,
            db_path: String::new(),
            recall_limit: 5,
            similarity_threshold: 0.5,
            time_decay_hours: 24.0,
            similarity_weight: 0.7,
            recency_weight: 0.3,
            auto_session_split: true,
            session_timeout_minutes: 30,
            topic_change_threshold: 0.5,
            min_turns_before_split: 3,
            summary_recall_limit: 3,
            tool_rag_enabled: true,
            tool_rag_limit: 6,
            tool_rag_always_include: vec![
                "question".to_string(),
                "todo".to_string(),
                "get_current_time".to_string(),
            ],
            summarization_model: String::new(),
            summarization_base_url: String::new(),
        }
    }
}

impl Default for AiSandboxSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            allowed_directories: vec![".".to_string()],
            writable_directories: vec![".".to_string()],
            blocked_commands: vec![
                r"rm\s+-rf\s+/".to_string(),
                r"dd\s+if=".to_string(),
                r"mkfs".to_string(),
                r"sudo\s+".to_string(),
                r":\s*\{\s*\|\s*&\s*;\s*\}".to_string(),
            ],
            max_read_bytes: 50 * 1024,
            max_write_bytes: 1024 * 1024,
            shell_timeout_ms: 120_000,
            max_shell_output_bytes: 50 * 1024,
            max_shell_output_lines: 2000,
        }
    }
}

impl AiSandboxSettings {
    /// `ene-tool-proto` 用のシリアライズ可能なサンドボックス設定に変換
    pub fn to_sandbox_config_data(&self, undo_db_path: Option<String>) -> ene_tool_proto::SandboxConfigData {
        ene_tool_proto::SandboxConfigData {
            enabled: self.enabled,
            allowed_directories: self.allowed_directories.clone(),
            writable_directories: self.writable_directories.clone(),
            blocked_commands: self.blocked_commands.clone(),
            max_read_bytes: self.max_read_bytes,
            max_write_bytes: self.max_write_bytes,
            shell_timeout_ms: self.shell_timeout_ms,
            max_shell_output_bytes: self.max_shell_output_bytes,
            max_shell_output_lines: self.max_shell_output_lines,
            undo_db_path,
        }
    }

}

impl AiSettings {
    /// Undo DB のパスを解決する（memory.db と同じディレクトリ）
    pub fn resolve_undo_db_path(&self) -> std::path::PathBuf {
        let memory_path = self.resolve_memory_db_path();
        memory_path.parent().unwrap_or(std::path::Path::new(".")).join("undo.db")
    }
}

impl Default for AiSettings {
    fn default() -> Self {
        Self {
            provider: AiProviderSettings::default(),
            character_card_path: String::new(),
            user_name: "User".to_string(),
            runtime_rules: "Keep responses relatively short and sweet, suitable for displaying on a screen overlay.".to_string(),
            tool_calling_enabled: true,
            max_tool_call_rounds: 10,
            mcp_servers: Vec::new(),
            embedding: AiEmbeddingSettings::default(),
            memory: AiMemorySettings::default(),
            sandbox: AiSandboxSettings::default(),
            tools: AiToolSettings::default(),
        }
    }
}

impl AiSettings {
    pub fn resolve_base_url(&self) -> Result<String, AiCoreError> {
        if self.provider.base_url.trim().is_empty() {
            return Err(AiCoreError::MissingBaseUrl {
                env_var: String::new(),
            });
        }
        Ok(self.provider.base_url.clone())
    }

    pub fn resolve_api_key(&self) -> String {
        if !self.provider.api_key.trim().is_empty() {
            return self.provider.api_key.clone();
        }
        #[cfg(debug_assertions)]
        {
            if let Ok(token) = std::env::var("API_TOKEN") {
                if !token.trim().is_empty() {
                    return token;
                }
            }
        }
        self.provider.api_key.clone()
    }

    pub fn resolve_memory_db_path(&self) -> std::path::PathBuf {
        if !self.memory.db_path.trim().is_empty() {
            return std::path::PathBuf::from(&self.memory.db_path);
        }
        let card_path = std::path::Path::new(&self.character_card_path);
        let dir = card_path.parent().unwrap_or(std::path::Path::new("."));
        dir.join("memory.db")
    }

    pub fn resolve_embedding_base_url(&self) -> Result<String, AiCoreError> {
        if !self.embedding.base_url.trim().is_empty() {
            return Ok(self.embedding.base_url.clone());
        }
        self.resolve_base_url()
    }

    /// 要約生成用のモデル名を解決する（空ならチャット用モデルを使用）
    pub fn resolve_summarization_model(&self) -> String {
        if !self.memory.summarization_model.trim().is_empty() {
            self.memory.summarization_model.clone()
        } else {
            self.provider.model.clone()
        }
    }

    /// 要約生成用の base_url を解決する（空ならチャット用 base_url を使用）
    pub fn resolve_summarization_base_url(&self) -> Result<String, AiCoreError> {
        if !self.memory.summarization_base_url.trim().is_empty() {
            return Ok(self.memory.summarization_base_url.clone());
        }
        self.resolve_base_url()
    }
}

/// `assets/settings.json` を読み込み、`character` フィールドから `character_card_path` を解決した
/// `AiSettings` を返す。
pub fn load_settings() -> AiSettings {
    let assets_dir = crate::paths::assets_dir();
    let config_path = crate::paths::config_file_path();
    load_settings_from(&assets_dir, &config_path)
}

/// 指定された `assets_dir` / `config_path` で設定を読み込む。
pub fn load_settings_from(assets_dir: &Path, config_path: &Path) -> AiSettings {
    load_full_settings_from(assets_dir, config_path).ai
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ShadowQuality {
    Low,
    #[default]
    Medium,
    High,
}

impl ShadowQuality {
    pub fn label(self) -> &'static str {
        match self {
            ShadowQuality::Low => "Low",
            ShadowQuality::Medium => "Medium",
            ShadowQuality::High => "High",
        }
    }

    pub fn shadow_map_size(self) -> usize {
        match self {
            ShadowQuality::Low => 1_024,
            ShadowQuality::Medium => 2_048,
            ShadowQuality::High => 4_096,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum AntialiasingMode {
    Off,
    #[default]
    Fxaa,
    Smaa,
    Taa,
}

impl AntialiasingMode {
    pub fn label(self) -> &'static str {
        match self {
            AntialiasingMode::Off => "Off",
            AntialiasingMode::Fxaa => "FXAA",
            AntialiasingMode::Smaa => "SMAA",
            AntialiasingMode::Taa => "TAA",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphicsSection {
    pub mask_render_downsample: u32,
    pub target_fps: u32,
    pub shadow_quality: ShadowQuality,
    pub antialiasing_mode: AntialiasingMode,
}

impl Default for GraphicsSection {
    fn default() -> Self {
        Self {
            mask_render_downsample: 8,
            target_fps: 60,
            shadow_quality: ShadowQuality::Medium,
            antialiasing_mode: AntialiasingMode::Fxaa,
        }
    }
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct AppSection {
    pub graphics: GraphicsSection,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppSettings {
    pub version: u32,
    pub character: String,
    pub app: AppSection,
    #[serde(flatten)]
    pub ai: AiSettings,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            version: 1,
            character: "Alicia".to_string(),
            app: AppSection::default(),
            ai: AiSettings::default(),
        }
    }
}

/// 設定ファイルを完全に読み込む
pub fn load_full_settings() -> AppSettings {
    let assets_dir = crate::paths::assets_dir();
    let config_path = crate::paths::config_file_path();
    load_full_settings_from(&assets_dir, &config_path)
}

/// 指定された `assets_dir` / `config_path` から AppSettings を完全に読み込む
pub fn load_full_settings_from(assets_dir: &Path, config_path: &Path) -> AppSettings {
    let mut settings = AppSettings::default();

    if let Ok(content) = std::fs::read_to_string(config_path) {
        if let Ok(s) = serde_json::from_str::<AppSettings>(&content) {
            settings = s;
        }
    }

    if settings.ai.character_card_path.trim().is_empty() {
        if !settings.character.trim().is_empty() {
            settings.ai.character_card_path = format!(
                "{}/characters/{}/character.json",
                assets_dir.display(),
                settings.character
            );
        } else {
            settings.ai.character_card_path = format!(
                "{}/characters/Alicia/character.json",
                assets_dir.display()
            );
        }
    }

    settings
}

/// 設定ファイルを型安全に保存する
pub fn save_full_settings(settings: &AppSettings) -> Result<(), std::io::Error> {
    let config_path = crate::paths::config_file_path();
    if let Some(parent) = config_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(settings)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    std::fs::write(config_path, json)?;
    Ok(())
}

