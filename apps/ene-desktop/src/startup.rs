//! Application startup phases for ene-desktop.
//!
//! | Phase | Function | When |
//! |-------|----------|------|
//! | 1 — First launch | [`first_launch_setup`] | Once per install (release asset copy) |
//! | 2 — App launch | [`load_desktop_settings`], [`init_app_state`] | Every process start (sync) |
//! | 3 — Runtime warmup | [`AiBridge`] background task via `ene_runtime::bootstrap_runtime` | Every start (async) |
//! | 4 — Graphics ready | [`crate::runtime::Runtime::resumed`] | After winit surface exists |

use std::path::PathBuf;

use crate::events::AppEventSender;
use crate::gpu::GpuContext;
use crate::settings::CharacterSettings;
use crate::state::{AppState, AppStateError};

/// Paths resolved during Phase 1 (first-launch asset deployment + CLI overrides).
pub struct FirstLaunchPaths {
    /// Absolute path to the assets directory.
    pub assets_dir: PathBuf,
    /// Default VRM path (CLI arg 1 or built-in default).
    pub default_vrm: String,
    /// Default VRMA path (CLI arg 2 or built-in default).
    #[expect(dead_code)]
    pub default_vrma: String,
}

/// Phase 1: deploy default assets on first launch (release) and read CLI overrides.
pub fn first_launch_setup() -> Result<FirstLaunchPaths, AppStateError> {
    let assets_dir =
        ene_config::ensure_resource_dirs().map_err(|e| AppStateError::AssetsDir(e.to_string()))?;
    let (default_vrm, default_vrma) = crate::settings::read_cli_paths();
    Ok(FirstLaunchPaths {
        assets_dir,
        default_vrm,
        default_vrma,
    })
}

/// Phase 2a: discover characters on disk and load persisted settings once.
pub fn load_desktop_settings(paths: &FirstLaunchPaths) -> CharacterSettings {
    CharacterSettings::discover(&paths.assets_dir, &paths.default_vrm)
}

/// Phase 2b: construct [`AppState`] with GPU context and the AI bridge.
pub fn init_app_state(
    gpu: GpuContext,
    settings: CharacterSettings,
    bootstrap_handle: &tokio::runtime::Handle,
) -> (AppState, AppEventSender) {
    AppState::with_channel(gpu, settings, bootstrap_handle)
}
