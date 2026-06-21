//! Per-process application state.
//!
//! [`AppState`] is constructed in `main` (where the tokio runtime is
//! alive) and handed to the winit [`Runtime`](crate::runtime::Runtime).
//! It owns the GPU context, settings, AI bridge, optional tray, the
//! event receiver, and the character renderer.
//!
//! Senders (clones of [`AppEventSender`](crate::events::AppEventSender))
//! are passed to producers at construction time so they can push
//! without holding a reference to the state.
use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::mpsc;

use crate::ai_bridge::AiBridge;
use crate::character::CharacterRenderer;
use crate::events::{AppEvent, AppEventReceiver, AppEventSender};
use crate::gpu::GpuContext;
use crate::settings::CharacterSettings;
use crate::tray::TrayHandle;

/// Linux-only display-server integration state.
///
/// On non-Linux targets every field is hidden behind
/// `#[cfg(target_os = "linux")]`; the struct is empty at compile
/// time, so the field access sites only need to gate the same way
/// they did when the fields lived directly on [`AppState`].
pub struct PlatformState {
    /// Wayland input-region context. `None` on non-Wayland displays;
    /// populated lazily in [`crate::runtime::Runtime::resumed`].
    #[cfg(target_os = "linux")]
    pub wayland_region:
        Option<Arc<parking_lot::Mutex<crate::platform::wayland_region::WaylandInputRegionContext>>>,
    /// X11 context for `_NET_WM_STATE_SKIP_TASKBAR` and the shape
    /// extension click-through.
    #[cfg(target_os = "linux")]
    pub x11_ctx: Option<Arc<parking_lot::Mutex<crate::platform::x11_taskbar::X11Context>>>,
    /// Wayland `zwlr_layer_shell_v1` detection context. Initialised
    /// alongside `wayland_region` so the click-through dispatcher can
    /// branch on layer-shell availability.
    #[cfg(target_os = "linux")]
    pub layer_shell: Option<crate::platform::wayland_layer_shell::LayerShellState>,
    /// `true` while the user holds the "freeze character window"
    /// hotkey (`F8`). Forces the xdg-shell fallback to receive all
    /// input even on compositors without layer-shell. Not persisted.
    #[cfg(target_os = "linux")]
    pub layer_shell_freeze: bool,
    /// Rectangles most recently pushed to the Wayland
    /// `wl_surface::set_input_region` and X11 `shape::rectangles`
    /// calls, in window-pixel space. Read by the F9 debug overlay.
    #[cfg(target_os = "linux")]
    pub last_applied_input_rects: Vec<crate::platform::wayland_region::Rect>,
    /// Which branch produced the rects. `Empty` when none were
    /// pushed (full pass-through).
    #[cfg(target_os = "linux")]
    pub last_input_source: crate::input_region_debug::InputRegionSource,

    /// Offscreen `Rgba8Unorm` mask capture target, drained each
    /// `about_to_wait` to feed silhouette rectangles into the
    /// Wayland input-region and X11 shape extension.
    #[cfg(target_os = "linux")]
    pub mask_capture: Option<crate::platform::wayland_mask_capture::MaskCaptureState>,
    /// Off-thread worker that drains the mask target without blocking
    /// the winit main thread.
    #[cfg(target_os = "linux")]
    pub mask_readback_worker: Option<crate::platform::mask_readback::MaskReadbackWorker>,
}

impl Default for PlatformState {
    fn default() -> Self {
        Self {
            #[cfg(target_os = "linux")]
            wayland_region: None,
            #[cfg(target_os = "linux")]
            x11_ctx: None,
            #[cfg(target_os = "linux")]
            layer_shell: None,
            #[cfg(target_os = "linux")]
            layer_shell_freeze: false,
            #[cfg(target_os = "linux")]
            last_applied_input_rects: Vec::new(),
            #[cfg(target_os = "linux")]
            last_input_source: crate::input_region_debug::InputRegionSource::Empty,
            #[cfg(target_os = "linux")]
            mask_capture: None,
            #[cfg(target_os = "linux")]
            mask_readback_worker: None,
        }
    }
}

/// Per-frame diagnostic state consumed by the F3 collider overlay
/// and the line-list renderer.
#[derive(Default)]
pub struct DebugState {
    /// Latest raycast hit from the click-through test, refreshed
    /// every `about_to_wait`. The character-window debug overlay
    /// reads this to draw the hit collider highlight and hit-point cross.
    pub last_raycast_hit: Option<crate::physics::RaycastHit>,
    /// Line-list overlay renderer, lazily created the first time
    /// the F3 toggle is on.
    pub debug_renderer: Option<ene_vrm::DebugRenderer>,
}

/// Container for the winit runtime.
pub struct AppState {
    pub gpu: GpuContext,
    pub settings: CharacterSettings,
    pub ai: Arc<AiBridge>,
    pub tray: Option<TrayHandle>,
    /// Receiver end of the cross-subsystem bus. The runtime drains
    /// this in `about_to_wait`.
    pub event_rx: AppEventReceiver,
    /// Character renderer (depth texture + default VRM).
    pub character: CharacterRenderer,
    /// ECS world for entities (character, camera, physics, etc).
    pub world: hecs::World,
    /// The primary character entity ID.
    pub character_entity: hecs::Entity,
    /// The UI state entity ID.
    pub ui_entity: hecs::Entity,
    /// Rapier physics state.
    pub physics: crate::physics::PhysicsWorld,
    /// Linux display-server integration state. Empty on non-Linux.
    pub platform: PlatformState,
    /// Debug overlay state (raycast hit + line-list renderer).
    pub debug: DebugState,
}

impl AppState {
    /// Construct the `AppState` together with a fresh `AppEvent`
    /// channel. The sender half is returned to the caller for
    /// optional auxiliary producers.
    ///
    /// The character renderer is **deferred** until the runtime
    /// creates the surface — it needs the actual surface format to
    /// build a compatible render pipeline. `with_channel` produces a
    /// `CharacterRenderer` that hasn't been `init`-ed yet;
    /// [`crate::runtime::Runtime::resumed`] calls
    /// [`CharacterRenderer::init`] right after the surface exists.
    pub fn with_channel(
        gpu: GpuContext,
        settings: CharacterSettings,
        bootstrap_handle: &tokio::runtime::Handle,
    ) -> (Self, AppEventSender) {
        let (tx, rx) = mpsc::unbounded_channel::<AppEvent>();
        let ai = Arc::new(AiBridge::new(tx.clone(), bootstrap_handle));
        let character =
            CharacterRenderer::uninit(&settings.assets_dir, settings.current_character());

        let mut world = hecs::World::new();
        let character_entity = world.spawn((crate::physics::Transform {
            translation: glam::Vec3::ZERO,
            scale: 1.0,
        },));
        let ui_entity = world.spawn((crate::settings::UiState::default(),));

        (
            Self {
                gpu,
                settings,
                ai,
                tray: None,
                event_rx: rx,
                character,
                world,
                character_entity,
                ui_entity,
                physics: crate::physics::PhysicsWorld::new(),
                platform: PlatformState::default(),
                debug: DebugState::default(),
            },
            tx,
        )
    }

    /// Initialise the tray. Safe to call multiple times; the second
    /// call is a no-op if the tray already exists.
    pub fn init_tray(&mut self, event_tx: &AppEventSender) {
        if self.tray.is_some() {
            return;
        }
        match TrayHandle::new(event_tx.clone()) {
            Some(handle) => self.tray = Some(handle),
            None => tracing::warn!("System tray failed to initialise; running headless"),
        }
    }

    /// Forward a one-shot user input string into the AI bridge.
    #[allow(dead_code)]
    pub fn ai_run(&self, input: impl Into<String>) {
        self.ai.run(input);
    }

    /// Persist current runtime state.
    pub fn save(&self) {
        self.settings.save();
    }

    /// Forward `Quit` into the bus. The runtime observes the next
    /// `about_to_wait` and calls `event_loop.exit()`.
    #[allow(dead_code)]
    pub fn request_quit(&self, event_tx: &AppEventSender) {
        let _ = event_tx.send(AppEvent::Quit);
    }

    pub fn ui_state(&self) -> hecs::Ref<'_, crate::settings::UiState> {
        self.world
            .get::<&crate::settings::UiState>(self.ui_entity)
            .expect("UI entity not found")
    }

    pub fn ui_state_mut(&self) -> hecs::RefMut<'_, crate::settings::UiState> {
        self.world
            .get::<&mut crate::settings::UiState>(self.ui_entity)
            .expect("UI entity not found")
    }
}

/// Path resolution + error type for `AppState` construction.
#[derive(Debug, thiserror::Error)]
pub enum AppStateError {
    #[error("GPU context failed to initialise: {0}")]
    Gpu(#[from] Box<dyn std::error::Error>),
    #[allow(dead_code)]
    #[error("Failed to resolve assets directory: {0}")]
    AssetsDir(String),
    #[error("Tokio runtime error: {0}")]
    Tokio(#[from] tokio::io::Error),
}

/// Read CLI overrides and return `(assets_dir, default_vrm,
/// default_vrma)`. `assets_dir` comes from
/// [`ene_config::ensure_resource_dirs`], which also creates the
/// directory if missing.
pub fn resolve_paths() -> Result<(PathBuf, String, String), AppStateError> {
    let assets_dir = ene_config::ensure_resource_dirs();
    let (default_vrm, default_vrma) = crate::settings::read_cli_paths();
    Ok((assets_dir, default_vrm, default_vrma))
}
