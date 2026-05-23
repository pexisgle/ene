pub mod character_card;
pub mod config;
pub mod error;
pub mod paths;
pub mod resources;

pub use character_card::{
    CharacterAsset, CharacterCardData, CharacterCardV3, ExpressionDefinition, ResolvedExpression,
    expand_cbs_macros, resolve_expressions,
};
pub use config::{
    EmbeddingSettings, MemorySettings, ProviderSettings, SandboxSettings, EneSettings,
    ToolSettings, AntialiasingMode, AppSection, AppSettings, EmbeddingProviderType,
    GraphicsSection, McpServerConfig, McpTransport, ShadowQuality, ToolEntry, load_full_settings,
    load_full_settings_from, load_settings, load_settings_from, save_full_settings,
};
pub use error::ConfigError;
pub use paths::{
    IS_DEV_BUILD, app_data_dir, assets_dir, builtin_tools_dir, config_file_path, models_dir,
    schema_file_path, tool_socket_dir, user_tools_dir,
};
pub use resources::ensure_resource_dirs;
