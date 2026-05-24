use std::path::PathBuf;

const APP_ID: &str = "dev.pexisgle.Ene";

pub const IS_DEV_BUILD: bool = cfg!(debug_assertions);

pub fn app_data_dir() -> PathBuf {
    directories::ProjectDirs::from("dev", "pexisgle", "Ene")
        .map(|proj_dirs| proj_dirs.data_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from(APP_ID))
}

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

pub fn models_dir() -> PathBuf {
    assets_dir().join("models")
}

pub fn config_file_path() -> PathBuf {
    assets_dir().join("settings.json")
}

pub fn schema_file_path() -> PathBuf {
    assets_dir().join("settings.schema.json")
}

/// ビルトインツールバイナリのディレクトリ
/// 実行ファイルと同じディレクトリ（デバッグ時）またはその `tools/` サブディレクトリ（リリース時）
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

/// ユーザー追加ツールのディレクトリ
/// app_data_dir()/tools/
pub fn user_tools_dir() -> PathBuf {
    app_data_dir().join("tools")
}

/// ツール用一時ソケットディレクトリ
pub fn tool_socket_dir() -> PathBuf {
    std::env::temp_dir().join(format!("{}.tools", APP_ID))
}

/// キャラクター固有の設定ファイルのパスを取得します。
/// assets_dir/characters/{name}/character_settings.json
pub fn character_settings_path(character_name: &str) -> PathBuf {
    assets_dir()
        .join("characters")
        .join(character_name)
        .join("character_settings.json")
}
