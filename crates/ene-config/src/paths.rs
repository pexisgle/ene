use std::path::PathBuf;

const APP_ID: &str = "dev.pexisgle.ene";

/// `true` when the binary was compiled in debug mode.
pub const IS_DEV_BUILD: bool = cfg!(debug_assertions);

/// Returns the OS-standard application data directory for ene.
pub fn app_data_dir() -> PathBuf {
    directories::ProjectDirs::from("dev", "pexisgle", "ene")
        .map(|proj_dirs| proj_dirs.data_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from(APP_ID))
}

/// Returns the assets directory.
///
/// In debug builds the source-tree `assets/` is used; in release builds
/// the app data directory is returned.
pub fn assets_dir() -> PathBuf {
    if cfg!(debug_assertions) {
        if let Some(exe_dir) = std::env::current_exe().ok().and_then(|e| e.parent().map(|p| p.to_path_buf())) {
            let candidates = [
                exe_dir.join("../../assets"),
                exe_dir.join("../assets"),
                PathBuf::from("assets"),
            ];
            if let Some(path) = candidates.into_iter().find(|c| c.is_dir()) {
                return path;
            }
        }
    }
    app_data_dir()
}

/// Returns the models directory (`assets/models`).
pub fn models_dir() -> PathBuf {
    assets_dir().join("models")
}

/// Returns the path to `settings.json`.
pub fn config_file_path() -> PathBuf {
    assets_dir().join("settings.json")
}

/// Returns the path to `settings.schema.json`.
pub fn schema_file_path() -> PathBuf {
    assets_dir().join("settings.schema.json")
}

/// Returns the path to `character_settings.schema.json`.
pub fn character_schema_file_path() -> PathBuf {
    assets_dir().join("character_settings.schema.json")
}

/// Directory for built-in tool binaries
/// Same directory as the executable (debug) or its `tools/` subdirectory (release)
pub fn builtin_tools_dir() -> PathBuf {
    if let Some(exe_dir) = std::env::current_exe().ok().and_then(|exe| exe.parent().map(|p| p.to_path_buf())) {
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
/// app_data_dir()/tools/
pub fn user_tools_dir() -> PathBuf {
    app_data_dir().join("tools")
}

/// Temporary socket directory for tools
pub fn tool_socket_dir() -> PathBuf {
    std::env::temp_dir().join(format!("{}.tools", APP_ID))
}

/// Gets the path to the character-specific settings file
/// assets_dir/characters/{name}/character_settings.json
pub fn character_settings_path(character_name: &str) -> PathBuf {
    assets_dir()
        .join("characters")
        .join(character_name)
        .join("character_settings.json")
}
