use std::path::PathBuf;

const APP_ID: &str = "dev.pexisgle.Ene";

pub const IS_DEV_BUILD: bool = cfg!(debug_assertions);

pub fn app_data_dir() -> PathBuf {
    dirs::data_dir()
        .map(|p| p.join(APP_ID))
        .unwrap_or_else(|| PathBuf::from(APP_ID))
}

pub fn assets_dir() -> PathBuf {
    source_assets_dir().unwrap_or_else(|| app_data_dir())
}

#[cfg(debug_assertions)]
fn source_assets_dir() -> Option<PathBuf> {
    let current_exe = std::env::current_exe().ok()?;
    let exe_dir = current_exe.parent()?;
    let candidates = [
        exe_dir.join("../../assets"),
        exe_dir.join("../assets"),
        PathBuf::from("assets"),
    ];
    candidates.into_iter().find(|c| c.is_dir())
}

#[cfg(not(debug_assertions))]
fn source_assets_dir() -> Option<PathBuf> {
    None
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
/// 実行ファイルと同じディレクトリの `tools/` サブディレクトリ
pub fn builtin_tools_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|p| p.join("tools")))
        .unwrap_or_else(|| PathBuf::from("tools"))
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
