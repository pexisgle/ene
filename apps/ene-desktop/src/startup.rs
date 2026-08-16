//! [`first_launch_setup`] runs once per install (release asset copy);
//! [`load_desktop_settings`] / [`init_app_state`] run at every process
//! start; async runtime warmup happens in [`AiBridge`] via
//! `ene_runtime::bootstrap_runtime`; the winit surface setup happens in
//! [`crate::runtime::Runtime::resumed`].

use std::path::PathBuf;

use crate::events::AppEventSender;
use crate::gpu::GpuContext;
use crate::settings::CharacterSettings;
use crate::state::{AppState, AppStateError};

/// Paths resolved during first-launch asset deployment + CLI overrides.
pub struct FirstLaunchPaths {
    pub assets_dir: PathBuf,
    /// CLI arg 1, else the built-in default.
    pub default_vrm: String,
    /// CLI arg 2, else the built-in default.
    #[expect(
        dead_code,
        reason = "default VRMA path retained for CLI override API completeness"
    )]
    pub default_vrma: String,
}

pub fn first_launch_setup() -> Result<FirstLaunchPaths, AppStateError> {
    let assets_dir =
        ene_config::ensure_resource_dirs().map_err(|e| AppStateError::AssetsDir(e.to_string()))?;

    // Write JSON schemas once at startup rather than on every config load.
    ene_config::write_schemas(&assets_dir);
    ene_card::write_character_schemas(&assets_dir);

    let (default_vrm, default_vrma) = crate::settings::read_cli_paths();
    Ok(FirstLaunchPaths {
        assets_dir,
        default_vrm,
        default_vrma,
    })
}

pub fn load_desktop_settings(paths: &FirstLaunchPaths) -> CharacterSettings {
    CharacterSettings::discover(&paths.assets_dir, &paths.default_vrm)
}

pub fn init_app_state(
    gpu: GpuContext,
    settings: CharacterSettings,
    bootstrap_handle: &tokio::runtime::Handle,
) -> (AppState, AppEventSender) {
    AppState::with_channel(gpu, settings, bootstrap_handle)
}
