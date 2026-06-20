//! Per-process application state.
//!
//! [`AppState`] is constructed in `main` (where the tokio runtime
//! is alive) and then handed to the winit [`Runtime`](crate::runtime::Runtime).
//! It owns:
//!
//! - the [`GpuContext`](crate::gpu::GpuContext) (instance / adapter / device / queue),
//! - the [`CharacterSettings`],
//! - the [`AiBridge`] (the actor handle plus its drain buffer),
//! - the optional [`TrayHandle`],
//! - the cross-subsystem [`AppEventReceiver`](crate::events::AppEventReceiver).
//! - the [`CharacterRenderer`](crate::character::CharacterRenderer) (PR3
//!   onward; loads the default VRM and owns the depth texture).
//!
//! Senders (clones of the [`AppEventSender`](crate::events::AppEventSender))
//! are passed into the AI bridge and the tray at construction time
//! so producers can push without holding a reference to the state.
use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::mpsc;

use crate::ai_bridge::AiBridge;
use crate::character::CharacterRenderer;
use crate::events::{AppEvent, AppEventReceiver, AppEventSender};
use crate::gpu::GpuContext;
use crate::settings::CharacterSettings;
use crate::tray::TrayHandle;

/// Container for the winit runtime.
pub struct AppState {
    pub gpu: GpuContext,
    pub settings: CharacterSettings,
    pub ai: Arc<AiBridge>,
    pub tray: Option<TrayHandle>,
    /// Receiver end of the cross-subsystem bus. The runtime drains
    /// this in `about_to_wait`.
    pub event_rx: AppEventReceiver,
    /// PR3 character renderer (loads the default VRM and owns the
    /// depth texture for the character window).
    pub character: CharacterRenderer,
    /// ECS World for entities (character, camera, physics, etc).
    pub world: hecs::World,
    /// The primary character entity ID
    pub character_entity: hecs::Entity,
    /// The UI state entity ID
    pub ui_entity: hecs::Entity,
    /// Rapier physics state
    pub physics: crate::physics::PhysicsWorld,
    /// PR5.6: latest raycast hit from the click-through test,
    /// refreshed every `about_to_wait` from
    /// [`crate::runtime::update_char_window_cursor_and_hittest`].
    /// The character-window debug overlay reads this to
    /// highlight the hit collider and draw the hit-point
    /// cross.
    pub last_raycast_hit: Option<crate::physics::RaycastHit>,
    /// PR5.6: line-list overlay renderer, lazily created
    /// the first time the F3 toggle is on (and the
    /// `surface_format` is final). `None` until then so
    /// the first frame is a clean redraw.
    pub debug_renderer: Option<ene_vrm::DebugRenderer>,
    /// PR5.3: Wayland input-region context for the character
    /// window. `None` when the underlying display is not
    /// Wayland (X11, macOS, Windows). Populated lazily in
    /// [`crate::runtime::Runtime::resumed`] once the winit
    /// window's raw handles resolve to a Wayland connection.
    #[cfg(target_os = "linux")]
    pub wayland_region:
        Option<Arc<parking_lot::Mutex<crate::platform::wayland_region::WaylandInputRegionContext>>>,
    /// PR5.4: X11 context for `_NET_WM_STATE_SKIP_TASKBAR`
    /// and the shape extension click-through. `None` until
    /// PR5.4 ships.
    #[cfg(target_os = "linux")]
    pub x11_ctx: Option<Arc<parking_lot::Mutex<crate::platform::x11_taskbar::X11Context>>>,
    /// PR5.4 / PR-LX.4: Wayland `zwlr_layer_shell_v1`
    /// detection context. `None` on non-Linux builds. The
    /// runtime initialises this alongside
    /// [`Self::wayland_region`] in
    /// [`crate::runtime::Runtime::resumed`] so the click-through
    /// dispatcher can branch on layer-shell availability.
    #[cfg(target_os = "linux")]
    pub layer_shell: Option<crate::platform::wayland_layer_shell::LayerShellState>,
    /// PR5.4 / PR-LX.4: `true` while the user is holding the
    /// "freeze character window" hotkey (`F8` by default). The
    /// xdg-shell fallback forces the window to receive all
    /// input when this is set, so the user can reach through to
    /// the character even on compositors without layer-shell.
    /// The flag is **not** persisted across launches.
    #[cfg(target_os = "linux")]
    pub layer_shell_freeze: bool,
    /// PR-LX.6: offscreen `Rgba8Unorm` mask capture target.
    /// Created in [`crate::runtime::Runtime::resumed`] once
    /// the GPU device is alive, sized in
    /// [`crate::runtime::Runtime::window_event`] on
    /// `Resized` / `ScaleFactorChanged`, and drained by
    /// [`crate::platform::platform_runtime::apply_linux_click_through`]
    /// each `about_to_wait` to feed silhouette rectangles into
    /// the Wayland input-region and X11 shape extension. The
    /// actual render pass that writes into the target view is
    /// wired in PR-LX.7; this field is initialised to `None`
    /// and populated by the runtime.
    #[cfg(target_os = "linux")]
    pub mask_capture: Option<crate::platform::wayland_mask_capture::MaskCaptureState>,
}

impl AppState {
    /// Construct the AppState together with a fresh `AppEvent`
    /// channel. The sender half is returned to the caller for
    /// optional auxiliary producers.
    ///
    /// The character renderer is **deferred** until the runtime
    /// creates the surface — it needs the actual surface format to
    /// build a compatible render pipeline. `with_channel` produces
    /// a `CharacterRenderer` that hasn't been `init`-ed yet;
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
                last_raycast_hit: None,
                debug_renderer: None,
                #[cfg(target_os = "linux")]
                wayland_region: None,
                #[cfg(target_os = "linux")]
                x11_ctx: None,
                #[cfg(target_os = "linux")]
                layer_shell: None,
                #[cfg(target_os = "linux")]
                layer_shell_freeze: false,
                #[cfg(target_os = "linux")]
                mask_capture: None,
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
    /// Mirrors the legacy Bevy `EneRequestEvent { user_input }`
    /// pathway.
    #[allow(dead_code)] // PR2 will call this from the AI page chat input.
    pub fn ai_run(&self, input: impl Into<String>) {
        self.ai.run(input);
    }

    /// Persist current runtime state. The legacy Bevy code calls
    /// `settings.save()` on F1-toggle-off, Escape, and
    /// `WindowCloseRequested`.
    pub fn save(&self) {
        self.settings.save();
    }

    /// Forward `Quit` into the bus. The runtime observes the next
    /// `about_to_wait` and calls `event_loop.exit()`.
    #[allow(dead_code)] // PR2 will call this from a "Quit" menu item.
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

/// Path resolution + error type for AppState construction. Lives
/// here because both `main` and the runtime need to report
/// construction failures uniformly.
#[derive(Debug, thiserror::Error)]
pub enum AppStateError {
    #[error("GPU context failed to initialise: {0}")]
    Gpu(#[from] Box<dyn std::error::Error>),
    #[allow(dead_code)] // Reserved for the PR1→PR2 path-resolution error variants.
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
