use std::path::{Path, PathBuf};

const APP_ID: &str = "dev.pexisgle.ene";

/// `true` when the binary was compiled in debug mode.
pub const IS_DEV_BUILD: bool = cfg!(debug_assertions);

/// OS-standard user data directory (`~/.local/share` on Linux, `%APPDATA%` on Windows).
pub fn app_data_dir() -> PathBuf {
    directories::ProjectDirs::from("dev", "pexisgle", "ene").map_or_else(
        || PathBuf::from(APP_ID),
        |proj_dirs| proj_dirs.data_dir().to_path_buf(),
    )
}

fn resolve_assets_dir_impl() -> PathBuf {
    if cfg!(debug_assertions)
        && let Some(exe_dir) = std::env::current_exe()
            .ok()
            .and_then(|e| e.parent().map(std::path::Path::to_path_buf))
    {
        let candidates = [
            exe_dir.join("../../assets"),
            exe_dir.join("../assets"),
            PathBuf::from("assets"),
        ];
        if let Some(path) = candidates.into_iter().find(|c| c.is_dir()) {
            return path;
        }
    }
    app_data_dir()
}

/// In debug builds the source-tree `assets/` is used; in release builds
/// the app data directory is returned.
///
/// Returns a `&'static Path` to avoid cloning the cached `PathBuf` on every call.
pub fn assets_dir() -> &'static std::path::Path {
    static CACHED: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    CACHED.get_or_init(resolve_assets_dir_impl).as_path()
}

/// VRM model assets directory.
pub fn models_dir() -> PathBuf {
    assets_dir().join("models")
}

/// User-editable settings file.
pub fn config_file_path() -> PathBuf {
    assets_dir().join("settings.json")
}

/// Runtime prompt pack for a language within an explicit base assets directory
/// (`base/lang/{code}/prompts.json`).
///
/// These packs are the runtime source of truth for [`crate::PromptLibrary`];
/// the compile-time embedded packs are only a fallback when this file is absent.
pub(crate) fn prompt_pack_path_in(base: &std::path::Path, language_code: &str) -> PathBuf {
    base.join("lang").join(language_code).join("prompts.json")
}

/// Runtime prompt pack for a language (`assets_dir/lang/{code}/prompts.json`).
pub fn prompt_pack_path(language_code: &str) -> PathBuf {
    prompt_pack_path_in(assets_dir(), language_code)
}

/// Runtime pattern pack for a language within an explicit base assets directory
/// (`base/lang/{code}/patterns.json`).
///
/// These packs are the runtime source of truth for [`crate::PatternLibrary`];
/// the compile-time embedded packs are only a fallback when this file is absent.
pub(crate) fn pattern_pack_path_in(base: &std::path::Path, language_code: &str) -> PathBuf {
    base.join("lang").join(language_code).join("patterns.json")
}

/// Runtime pattern pack for a language (`assets_dir/lang/{code}/patterns.json`).
pub fn pattern_pack_path(language_code: &str) -> PathBuf {
    pattern_pack_path_in(assets_dir(), language_code)
}

/// JSON Schema for `settings.json`.
pub fn schema_file_path() -> PathBuf {
    assets_dir().join("schema").join("settings.schema.json")
}

/// JSON Schema for character-specific settings.
pub fn character_schema_file_path() -> PathBuf {
    assets_dir()
        .join("schema")
        .join("character_settings.schema.json")
}

/// JSON Schema for the character card format.
pub fn character_card_schema_file_path() -> PathBuf {
    assets_dir().join("schema").join("character.schema.json")
}

/// Directory for built-in tool binaries
/// Same directory as the executable (debug) or its `tools/` subdirectory (release)
pub fn builtin_tools_dir() -> PathBuf {
    if let Some(exe_dir) = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(std::path::Path::to_path_buf))
    {
        if cfg!(debug_assertions) {
            exe_dir
        } else {
            exe_dir.join("tools")
        }
    } else {
        PathBuf::from("tools")
    }
}

/// Directory for user-added tools
/// `app_data_dir()/tools`/
pub fn user_tools_dir() -> PathBuf {
    app_data_dir().join("tools")
}

/// Temporary socket directory for tools
pub fn tool_socket_dir() -> PathBuf {
    std::env::temp_dir().join(format!("{APP_ID}.tools"))
}

/// Directory for built-in plugin binaries.
/// Same directory as the executable (debug) or its `plugins/` subdirectory (release).
pub fn builtin_plugins_dir() -> PathBuf {
    if let Some(exe_dir) = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(std::path::Path::to_path_buf))
    {
        if cfg!(debug_assertions) {
            exe_dir
        } else {
            exe_dir.join("plugins")
        }
    } else {
        PathBuf::from("plugins")
    }
}

/// Directory for user-added plugins.
/// `app_data_dir()/plugins`/
pub fn user_plugins_dir() -> PathBuf {
    app_data_dir().join("plugins")
}

/// Temporary socket directory for plugins.
pub fn plugin_socket_dir() -> PathBuf {
    std::env::temp_dir().join(format!("{APP_ID}.plugins"))
}

/// Gets the path to the character-specific settings file
/// `assets_dir/characters/{name}/character_settings.json`
pub fn character_settings_path(character_name: &str) -> PathBuf {
    character_settings_path_in(assets_dir(), character_name)
}

/// `character_settings_path` for an explicit base assets directory.
pub fn character_settings_path_in(base: &Path, character_name: &str) -> PathBuf {
    character_dir_in(base, character_name).join("character_settings.json")
}

/// Returns the directory containing the character card and runtime data
/// (`assets_dir/characters/{name}/`).
pub fn character_dir(character_name: &str) -> PathBuf {
    character_dir_in(assets_dir(), character_name)
}

/// `character_dir` for an explicit base assets directory.
pub fn character_dir_in(base: &Path, character_name: &str) -> PathBuf {
    base.join("characters").join(character_name)
}
