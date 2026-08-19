use std::path::{Path, PathBuf};

const APP_ID: &str = "dev.pexisgle.ene";

pub const IS_DEV_BUILD: bool = cfg!(debug_assertions);

/// OS-standard user data directory (`~/.local/share` on Linux, `%APPDATA%` on Windows).
///
/// Release fallback for [`assets_dir`] only. Runtime code should call
/// [`data_dir`] so debug builds stay in source-tree `assets/` and never
/// touch this path.
pub fn app_data_dir() -> PathBuf {
    directories::ProjectDirs::from("dev", "pexisgle", "ene").map_or_else(
        || PathBuf::from(APP_ID),
        |proj_dirs| proj_dirs.data_dir().to_path_buf(),
    )
}

/// Runtime root for `settings.json`, databases, vault, and workspace.
///
/// `ENE_DATA_DIR` overrides when set. Otherwise this is [`assets_dir`]:
/// debug builds use source-tree `assets/`; release builds use
/// [`app_data_dir`].
#[must_use]
pub fn data_dir() -> PathBuf {
    resolve_data_dir(std::env::var("ENE_DATA_DIR").ok().as_deref(), assets_dir())
}

fn resolve_data_dir(ene_data_dir: Option<&str>, assets: &Path) -> PathBuf {
    match ene_data_dir {
        Some(dir) => PathBuf::from(dir),
        None => assets.to_path_buf(),
    }
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
            return path.canonicalize().unwrap_or(path);
        }
    }
    app_data_dir()
}

/// Debug: source-tree `assets/`. Release: [`app_data_dir`] (never the
/// repository `assets/` folder).
///
/// Returns a `&'static Path` to avoid cloning the cached `PathBuf` on every call.
pub fn assets_dir() -> &'static std::path::Path {
    static CACHED: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    CACHED.get_or_init(resolve_assets_dir_impl).as_path()
}

pub fn models_dir() -> PathBuf {
    assets_dir().join("models")
}

pub fn config_file_path() -> PathBuf {
    data_dir().join("settings.json")
}

/// Runtime prompt pack for a language within an explicit base assets directory
/// (`base/lang/{code}/prompts.json`).
///
/// These packs are the runtime source of truth for [`crate::PromptLibrary`];
/// the compile-time embedded packs are only a fallback when this file is absent.
pub(crate) fn prompt_pack_path_in(base: &std::path::Path, language_code: &str) -> PathBuf {
    base.join("lang").join(language_code).join("prompts.json")
}

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

pub fn pattern_pack_path(language_code: &str) -> PathBuf {
    pattern_pack_path_in(assets_dir(), language_code)
}

pub fn schema_file_path() -> PathBuf {
    assets_dir().join("schema").join("settings.schema.json")
}

pub fn character_schema_file_path() -> PathBuf {
    assets_dir()
        .join("schema")
        .join("character_settings.schema.json")
}

pub fn character_card_schema_file_path() -> PathBuf {
    assets_dir().join("schema").join("character.schema.json")
}

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

pub fn user_tools_dir() -> PathBuf {
    data_dir().join("tools")
}

pub fn tool_socket_dir() -> PathBuf {
    std::env::temp_dir().join(format!("{APP_ID}.tools"))
}

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

pub fn user_plugins_dir() -> PathBuf {
    data_dir().join("plugins")
}

pub fn plugin_socket_dir() -> PathBuf {
    socket_dir_for(&std::env::temp_dir())
}

fn socket_dir_for(temp: &Path) -> PathBuf {
    let preferred = temp.join(format!("{APP_ID}.plugins"));
    #[cfg(unix)]
    {
        // Unix sockets fail to bind once the path exceeds SUN_LEN (108 on
        // Linux, 104 on macOS); long TMPDIR values (nested sandbox homes)
        // must not break the plugin host, so fall back to /tmp for sockets.
        if preferred.as_os_str().len() > 70 {
            return PathBuf::from("/tmp").join(format!("{APP_ID}.plugins"));
        }
    }
    preferred
}

pub fn character_settings_path(character_name: &str) -> PathBuf {
    character_settings_path_in(assets_dir(), character_name)
}

pub fn character_settings_path_in(base: &Path, character_name: &str) -> PathBuf {
    character_dir_in(base, character_name).join("character_settings.json")
}

/// Returns the directory containing the character card and runtime data
/// (`assets_dir/characters/{name}/`).
pub fn character_dir(character_name: &str) -> PathBuf {
    character_dir_in(assets_dir(), character_name)
}

pub fn character_dir_in(base: &Path, character_name: &str) -> PathBuf {
    base.join("characters").join(character_name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_file_path_is_under_data_dir() {
        assert_eq!(config_file_path(), data_dir().join("settings.json"));
    }

    #[test]
    fn user_plugin_and_tool_dirs_are_under_data_dir() {
        let root = data_dir();
        assert_eq!(user_plugins_dir(), root.join("plugins"));
        assert_eq!(user_tools_dir(), root.join("tools"));
    }

    #[test]
    fn resolve_data_dir_prefers_override_then_assets() {
        let assets = Path::new("/repo/assets");
        assert_eq!(
            resolve_data_dir(None, assets),
            PathBuf::from("/repo/assets")
        );
        assert_eq!(
            resolve_data_dir(Some("/tmp/ene-data"), assets),
            PathBuf::from("/tmp/ene-data")
        );
    }

    #[test]
    fn canonicalize_drops_dotdot_from_debug_assets_join() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let assets = tmp.path().join("assets");
        std::fs::create_dir(&assets).expect("assets dir");
        let nested = tmp.path().join("target").join("debug");
        std::fs::create_dir_all(&nested).expect("target/debug");
        let joined = nested.join("../../assets");
        assert!(
            joined.to_string_lossy().contains(".."),
            "join should keep .. components: {}",
            joined.display()
        );
        let canon = joined.canonicalize().expect("canonicalize joined");
        assert_eq!(canon, assets.canonicalize().expect("canonicalize assets"));
        assert!(!canon.to_string_lossy().contains(".."));
    }

    #[test]
    fn short_temp_dir_is_used_as_is() {
        let dir = socket_dir_for(Path::new("/tmp"));
        assert_eq!(dir, PathBuf::from("/tmp/dev.pexisgle.ene.plugins"));
    }

    #[test]
    fn long_temp_dir_falls_back_to_short_socket_path() {
        let long = Path::new("/home/someone/.local/state/very/long/temp/path");
        let dir = socket_dir_for(long);
        assert!(dir.as_os_str().len() <= 70, "socket dir too long: {dir:?}");
        #[cfg(unix)]
        assert_eq!(dir, PathBuf::from("/tmp/dev.pexisgle.ene.plugins"));
    }
}
