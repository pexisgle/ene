//! Winit event loop driver.
//!
//! Owns the winit window(s), their wgpu surfaces, and the
//! [`AppState`]. Implements winit 0.30's `ApplicationHandler` so
//! `EventLoop::run_app` is a single call from `main`.
//!
//! Redraws from `about_to_wait` instead of `RedrawRequested` to avoid
//! winit 0.30's double-fire on Windows.
use std::sync::Arc;
use std::time::{Duration, Instant};

use winit::application::ApplicationHandler;
use winit::dpi::{LogicalPosition, LogicalSize, PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, KeyEvent, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Fullscreen, Window, WindowAttributes, WindowId, WindowLevel};

use crate::acquire_error::AcquireError;
use crate::ai_bridge::AiBridge;
use crate::caption_overlay::CaptionOverlayWindow;
use crate::chat_ui::ChatEguiWindow;
use crate::events::AppEventSender;
use crate::gpu::{WindowSurfaceError, pick_format_and_alpha};
use crate::hotkey::HotkeyState;
use crate::settings::{CharacterSettings, DesktopSection, DesktopThemePreference};
use crate::settings_ui::{PageKind, SettingsUi, widgets::SettingsAction};
use crate::spotlight::SpotlightWindow;
use crate::state::AppState;
use bevy_ecs::entity::Entity;
use device_query::DeviceQuery;

/// Framing margin passed to `Character::auto_fit_scale` when fitting the
/// avatar to the camera. A value of `0.9` leaves a 10% margin around the
/// model's normalized bounds so it is not flush against the viewport edges.
const CHARACTER_AUTO_FIT_MARGIN: f32 = 0.9;

/// Top-level runtime. One per process.
pub struct Runtime {
    state: AppState,
    event_tx: AppEventSender,
    transparent: bool,
    char_window: Option<CharacterWindow>,
    ui_window: Option<UiWindow>,
    chat_egui_window: Option<ChatEguiWindow>,
    spotlight_window: Option<SpotlightWindow>,
    caption_window: Option<CaptionOverlayWindow>,
    last_cursor_physical: Option<PhysicalPosition<f64>>,
    last_frame_instant: Option<Instant>,
    /// Monotonic clock origin for the emotion pipeline, shared with the TTS
    /// playback thread (`AppState::clock_origin`). `tick_emotions` reads
    /// `elapsed()` from this base and the playback thread schedules
    /// TTS-synced cues on the same base, so a cue pops exactly when the
    /// matching sentence's audio starts.
    emotion_clock: Instant,
    device_state: device_query::DeviceState,
    char_surface_fatal: bool,
    /// Whether an Alt modifier key is currently held. Tracked from
    /// `WindowEvent::ModifiersChanged` so the Alt+Space spotlight
    /// hotkey can be detected on `Space` key presses.
    alt_held: bool,
    /// Global Alt+Space registration; `None` on Wayland or when the
    /// registration is taken, in which case in-window handling stays on.
    hotkey: HotkeyState,
    /// Theme preference last pushed to every window. `None` means the first
    /// reconciliation has not happened; `Some(System)` maps to no native
    /// override so winit keeps reporting OS theme changes on Windows.
    native_theme_preference: Option<DesktopThemePreference>,
    /// Previous frame's `tts_playing` value, used to detect the
    /// true→false transition and reset the viseme mouth shape.
    #[cfg(feature = "voice")]
    prev_tts_playing: bool,
    /// Single long-lived microphone capture handle shared by the chat
    /// UI and the spotlight mic action. Lives here (not in a window)
    /// because `cpal::Stream` is `!Send + !Sync` and dropping a window
    /// would otherwise stop capture.
    mic_handle: crate::audio::MicCaptureHandle,
}

impl Runtime {
    pub fn new(state: AppState, event_tx: AppEventSender) -> Self {
        let mut state = state;
        let emotion_clock = state.clock_origin;
        // Run the bevy schedule once eagerly so the `Startup`
        // systems (notably `UiPlugin::spawn_settings_ui_window`)
        // can spawn the UI entity before any winit callback
        // tries to read `UiStateComponent`. Without this, the
        // first `resumed` panics on
        // `state.ui_bevy_state()` because the entity hasn't
        // been spawned yet — `app.update()` in `about_to_wait`
        // runs strictly *after* the first `resumed`.
        state.app.update();

        if ene_ai::needs_onboarding(&state.settings.config()) {
            let mut ui = state.ui_bevy_state_mut();
            ui.0.show_onboarding = true;
            ui.0.focused_page = Some(crate::settings_ui::PageKind::Ai);
            ui.0.settings_window_visible = true;
        }

        let startup_error = state
            .app
            .world_mut()
            .remove_resource::<crate::resource::startup::RuntimeStartupError>()
            .and_then(|resource| resource.0);
        if let Some(error) = startup_error {
            let mut ui = state.ui_bevy_state_mut();
            ui.0.settings_window_visible = true;
            ui.0.fatal_startup_dismissed = false;
            ui.0.runtime_startup_error = Some(error);
        }

        let mut hotkey = HotkeyState::new();
        if hotkey.is_registered() {
            tracing::info!("Global Alt+Space hotkey registered");
        }
        // A persisted `spotlight_enabled = false` must not grab
        // Alt+Space even briefly at startup; the per-frame reconcile
        // would release it on the first frame anyway.
        if !state.settings.spotlight_enabled()
            && let Err(error) = hotkey.sync_enabled(false)
        {
            tracing::warn!(error = %error, "Failed to release global hotkey at startup");
        }
        crate::theme::set_preference(state.settings.theme());
        #[cfg(target_os = "linux")]
        crate::theme::spawn_os_theme_watch();

        Self {
            state,
            event_tx,
            transparent: true,
            char_window: None,
            ui_window: None,
            chat_egui_window: None,
            spotlight_window: None,
            caption_window: None,
            last_cursor_physical: None,
            last_frame_instant: None,
            emotion_clock,
            device_state: device_query::DeviceState::new(),
            char_surface_fatal: false,
            alt_held: false,
            hotkey,
            native_theme_preference: None,
            #[cfg(feature = "voice")]
            prev_tts_playing: false,
            mic_handle: None,
        }
    }

    fn create_ui_window(&mut self, event_loop: &ActiveEventLoop) {
        let ui_attrs = WindowAttributes::default()
            .with_title("ene UI")
            .with_inner_size(LogicalSize::new(900.0, 700.0))
            .with_min_inner_size(LogicalSize::new(520.0, 560.0))
            .with_resizable(true);
        let ui_w = match event_loop.create_window(ui_attrs) {
            Ok(w) => Arc::new(w),
            Err(e) => {
                tracing::error!("Failed to create ui window: {e}");
                event_loop.exit();
                return;
            }
        };
        apply_window_theme(&ui_w, self.state.settings.theme());
        let ui_size = ui_w.inner_size();
        match UiWindow::new(
            ui_w,
            &self.state.gpu.instance,
            &self.state.gpu.adapter,
            &self.state.gpu.device,
            ui_size,
        ) {
            Ok(uw) => {
                let visible = self.state.ui_bevy_state().0.settings_window_visible;
                uw.window.set_visible(visible);
                if visible {
                    uw.window.request_redraw();
                }
                self.ui_window = Some(uw);
            }
            Err(e) => {
                tracing::error!("Failed to create UiWindow: {e}");
                event_loop.exit();
            }
        }
    }

    fn show_settings_window(&mut self, _event_loop: &ActiveEventLoop) {
        let visible = self.state.ui_bevy_state().0.settings_window_visible;
        if !visible {
            self.state
                .ui_bevy_state_mut()
                .0
                .character_editor_close_requested = false;
            self.state.ui_bevy_state_mut().0.settings_close_requested = false;
            self.state.ui_bevy_state_mut().0.settings_window_visible = true;
        }
    }

    fn hide_settings_window(&mut self) {
        // Unsaved character-card edits must survive an accidental close: hold
        // the window open and let the editor's discard dialog decide.
        if self.state.ui_bevy_state().0.editor_has_unsaved_changes() {
            self.state
                .ui_bevy_state_mut()
                .0
                .character_editor_close_requested = true;
            if let Some(uw) = self.ui_window.as_ref() {
                uw.window.request_redraw();
            }
            return;
        }
        // Pending draft edits must survive an accidental close too: hold the
        // window open and let the settings discard dialog decide.
        if self
            .ui_window
            .as_ref()
            .is_some_and(|uw| uw.settings_ui.draft.is_dirty())
        {
            self.state.ui_bevy_state_mut().0.settings_close_requested = true;
            if let Some(uw) = self.ui_window.as_ref() {
                uw.window.request_redraw();
            }
            return;
        }
        let visible = self.state.ui_bevy_state().0.settings_window_visible;
        if visible {
            self.state.ui_bevy_state_mut().0.settings_close_requested = false;
            self.state.save();
            self.state.ui_bevy_state_mut().0.settings_window_visible = false;
            // Drop the window entirely to work around Wayland/winit 0.30 unmap bugs.
            self.ui_window = None;
        }
    }

    /// User-initiated app exit (main-window close or Esc). With unsaved
    /// character-card edits the exit is deferred to the discard dialog;
    /// otherwise it happens immediately. Fatal-error exits keep their direct
    /// `event_loop.exit()` paths.
    fn request_app_exit(&mut self, event_loop: &ActiveEventLoop) {
        if self.state.ui_bevy_state().0.editor_has_unsaved_changes() {
            let needs_window = {
                let mut ui_state = self.state.ui_bevy_state_mut();
                ui_state.0.app_exit_requested = true;
                let was_hidden = !ui_state.0.settings_window_visible;
                ui_state.0.settings_window_visible = true;
                was_hidden
            };
            if needs_window && self.ui_window.is_none() {
                self.create_ui_window(event_loop);
            }
            return;
        }
        self.state.save();
        event_loop.exit();
    }

    fn create_chat_window(&mut self, event_loop: &ActiveEventLoop) {
        let mut chat_attrs = WindowAttributes::default()
            .with_title(i18n_embed_fl::fl!(
                crate::i18n::loader(),
                "chat-window-title"
            ))
            .with_inner_size(LogicalSize::new(400.0, 600.0))
            .with_resizable(true);
        if let Some(monitor) = event_loop.primary_monitor() {
            let monitor_size = monitor.size();
            let width = 400.0;
            let height = 600.0;
            let x = f64::from(monitor_size.width) - width - 16.0;
            let y = f64::from(monitor_size.height) - height - 48.0;
            chat_attrs = chat_attrs
                .with_position(PhysicalPosition::new(x.max(0.0) as i32, y.max(0.0) as i32));
        }
        let chat_w = match event_loop.create_window(chat_attrs) {
            Ok(w) => Arc::new(w),
            Err(e) => {
                tracing::error!("Failed to create chat window: {e}");
                return;
            }
        };
        apply_window_theme(&chat_w, self.state.settings.theme());
        let chat_size = chat_w.inner_size();
        match ChatEguiWindow::new(
            chat_w,
            &self.state.gpu.instance,
            &self.state.gpu.adapter,
            &self.state.gpu.device,
            chat_size,
        ) {
            Ok(cw) => {
                let visible = self.state.chat_bevy_state().0.chat_window_visible;
                cw.window.set_visible(visible);
                if visible {
                    cw.window.request_redraw();
                }
                self.chat_egui_window = Some(cw);
            }
            Err(e) => tracing::error!("Failed to create ChatEguiWindow: {e}"),
        }
    }

    fn show_chat_window(&mut self) {
        if !self.state.chat_bevy_state().0.chat_window_visible {
            self.state.chat_bevy_state_mut().0.chat_window_visible = true;
        }
    }

    fn hide_chat_window(&mut self) {
        if self.state.chat_bevy_state().0.chat_window_visible {
            self.state.chat_bevy_state_mut().0.chat_window_visible = false;
            self.chat_egui_window = None;
        }
    }

    fn toggle_spotlight(&mut self, event_loop: &ActiveEventLoop) {
        if !self.state.settings.spotlight_enabled() {
            return;
        }
        let next = {
            let mut ui_state = self.state.ui_bevy_state_mut();
            ui_state.0.spotlight_visible = !ui_state.0.spotlight_visible;
            if ui_state.0.spotlight_visible {
                ui_state.0.spotlight_input.clear();
                ui_state.0.spotlight_selection = 0;
            }
            ui_state.0.spotlight_visible
        };
        if !next {
            self.spotlight_window = None;
            return;
        }
        if self.spotlight_window.is_none() {
            self.create_spotlight_window(event_loop);
        }
        if let Some(w) = self.spotlight_window.as_ref() {
            w.window.request_redraw();
        }
    }

    fn create_spotlight_window(&mut self, event_loop: &ActiveEventLoop) {
        let mut attrs = WindowAttributes::default()
            .with_title(i18n_embed_fl::fl!(crate::i18n::loader(), "spotlight-title"))
            .with_inner_size(LogicalSize::new(460.0, 340.0))
            .with_resizable(false)
            .with_decorations(false)
            .with_transparent(true)
            .with_window_level(WindowLevel::AlwaysOnTop);
        if let Some(monitor) = event_loop.primary_monitor() {
            let position = monitor.position();
            let size = monitor.size();
            let scale = monitor.scale_factor();
            let width = (460.0 * scale) as i32;
            attrs = attrs.with_position(PhysicalPosition::new(
                position.x + size.width as i32 / 2 - width / 2,
                position.y + size.height as i32 / 5,
            ));
        }
        let spotlight_w = match event_loop.create_window(attrs) {
            Ok(w) => Arc::new(w),
            Err(e) => {
                tracing::error!("Failed to create spotlight window: {e}");
                return;
            }
        };
        apply_window_theme(&spotlight_w, self.state.settings.theme());
        let size = spotlight_w.inner_size();
        match SpotlightWindow::new(
            spotlight_w,
            &self.state.gpu.instance,
            &self.state.gpu.adapter,
            &self.state.gpu.device,
            size,
        ) {
            Ok(w) => self.spotlight_window = Some(w),
            Err(e) => tracing::error!("Failed to create SpotlightWindow: {e}"),
        }
    }

    fn create_caption_window(&mut self, event_loop: &ActiveEventLoop) {
        let mut attrs = WindowAttributes::default()
            .with_title(i18n_embed_fl::fl!(crate::i18n::loader(), "caption-title"))
            .with_inner_size(LogicalSize::new(520.0, 140.0))
            .with_resizable(false)
            .with_decorations(false)
            .with_transparent(true)
            .with_window_level(WindowLevel::AlwaysOnTop);
        let desktop_position = self.state.settings.config_section::<DesktopSection>();
        let (position, pinned) = {
            let ui_state = self.state.ui_bevy_state();
            (
                ui_state
                    .0
                    .caption_position
                    .or(desktop_position.caption_position),
                ui_state
                    .0
                    .caption_pinned
                    .or(desktop_position.caption_pinned)
                    .unwrap_or(true),
            )
        };
        if let Some((x, y)) = position {
            attrs = attrs.with_position(LogicalPosition::new(x, y));
        } else if let Some(monitor) = event_loop.primary_monitor() {
            let position = monitor.position();
            let size = monitor.size();
            let scale = monitor.scale_factor();
            let width = (520.0 * scale) as i32;
            let height = (140.0 * scale) as i32;
            attrs = attrs.with_position(PhysicalPosition::new(
                position.x + size.width as i32 / 2 - width / 2,
                position.y + size.height as i32 - height - 48,
            ));
        }
        if !pinned {
            attrs = attrs.with_window_level(WindowLevel::Normal);
        }
        let caption_w = match event_loop.create_window(attrs) {
            Ok(w) => Arc::new(w),
            Err(e) => {
                tracing::error!("Failed to create caption window: {e}");
                return;
            }
        };
        apply_window_theme(&caption_w, self.state.settings.theme());
        let size = caption_w.inner_size();
        match CaptionOverlayWindow::new(
            caption_w,
            &self.state.gpu.instance,
            &self.state.gpu.adapter,
            &self.state.gpu.device,
            size,
            pinned,
        ) {
            Ok(w) => self.caption_window = Some(w),
            Err(e) => tracing::error!("Failed to create CaptionOverlayWindow: {e}"),
        }
    }
}

impl ApplicationHandler for Runtime {
    /// Create the winit windows, init GPU surfaces, load the VRM, and set
    /// up platform-specific click-through / tray.
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        event_loop.set_control_flow(ControlFlow::Poll);

        self.state.init_tray(&self.event_tx);

        if self.char_window.is_some() {
            return;
        }

        let fullscreen = Some(Fullscreen::Borderless(None));
        let char_attrs = window_attributes(self.transparent, fullscreen);
        let char_w = match event_loop.create_window(char_attrs) {
            Ok(w) => Arc::new(w),
            Err(e) => {
                tracing::error!("Failed to create character window: {e}");
                event_loop.exit();
                return;
            }
        };
        #[cfg(target_os = "windows")]
        crate::theme::observe_winit_system_theme(&char_w);
        apply_window_theme(&char_w, self.state.settings.theme());
        let char_size = char_w.inner_size();
        match CharacterWindow::new(
            char_w,
            &self.state.gpu.instance,
            &self.state.gpu.adapter,
            &self.state.gpu.device,
            char_size,
        ) {
            Ok(cw) => {
                let format = cw.config.format;

                #[cfg(target_os = "linux")]
                let mask_format = Some(crate::platform::wayland_mask_capture::MASK_TARGET_FORMAT);
                #[cfg(not(target_os = "linux"))]
                let mask_format = None;

                self.state.character.init(
                    &self.state.gpu.device,
                    &self.state.gpu.queue,
                    format,
                    mask_format,
                );
                self.state
                    .character
                    .resize(&self.state.gpu.device, (char_size.width, char_size.height));

                #[cfg(target_os = "linux")]
                {
                    use crate::resource::platform_state::resources::{
                        LayerShell, LayerShellFreeze, MaskCapture, MaskReadbackWorkerRes,
                        WaylandInputRegion, X11ContextRes,
                    };
                    let world = self.state.app.world_mut();
                    if world.resource::<WaylandInputRegion>().0.is_none()
                        && let Some(ctx) =
                            crate::platform::wayland_region::WaylandInputRegionContext::try_new(
                                cw.window.as_ref(),
                            )
                    {
                        world.insert_resource(WaylandInputRegion(Some(ctx)));
                    }
                    if world.resource::<LayerShell>().0.is_none() {
                        let ls = crate::platform::wayland_layer_shell::new_layer_shell_state();
                        let status = crate::platform::detect_layer_shell(
                            Some(&ls),
                            world.resource::<WaylandInputRegion>().0.as_ref(),
                        );
                        world.insert_resource(LayerShell(Some(ls)));
                        tracing::info!(
                            target: "ene.linux.layer_shell",
                            available = matches!(
                                status,
                                crate::platform::wayland_layer_shell::LayerShellStatus::Available(_)
                            ),
                            "zwlr_layer_shell_v1 detection"
                        );
                    }
                    if world.resource::<MaskCapture>().0.is_none() {
                        let downsample = self
                            .state
                            .settings
                            .graphics()
                            .resolved()
                            .mask_render_downsample;
                        if let Some(cam) =
                            crate::platform::wayland_mask_capture::new_mask_capture_state(
                                &self.state.gpu.device,
                                char_size.width,
                                char_size.height,
                                downsample,
                            )
                        {
                            let device = Arc::new(self.state.gpu.device.clone());
                            let queue = Arc::new(self.state.gpu.queue.clone());
                            let worker = crate::platform::mask_readback::MaskReadbackWorker::spawn(
                                Arc::clone(&cam),
                                device,
                                queue,
                            );
                            world.insert_resource(MaskReadbackWorkerRes(Some(worker)));
                            world.insert_resource(MaskCapture(Some(cam)));
                        }
                    }
                    if !world.resource::<LayerShellFreeze>().0 {
                        world.insert_resource(LayerShellFreeze(false));
                    }
                    if world.resource::<X11ContextRes>().0.is_none()
                        && let Some(ctx) =
                            crate::platform::x11_taskbar::X11Context::try_new(cw.window.as_ref())
                    {
                        world.insert_resource(X11ContextRes(Some(ctx)));
                        tracing::info!(
                            target: "ene.linux.x11",
                            connected = true,
                            "X11 probe"
                        );
                    }
                }

                #[cfg(target_os = "windows")]
                {
                    let actual_scale = self
                        .state
                        .character
                        .auto_fit_scale(CHARACTER_AUTO_FIT_MARGIN)
                        * self.state.settings.character_state.model_scale;
                    let specs = self
                        .state
                        .character
                        .build_character_bone_specs(actual_scale);
                    if !specs.is_empty() {
                        let registration = self.state.with_physics_world_mut(|physics| {
                            physics.register_character_colliders(&specs)
                        });
                        self.state.character_physics_registration = Some(registration);
                    }
                }

                if let Some(motion_rel) = self.state.settings.current_motion() {
                    let motion_path = self.state.settings.assets_dir.join(motion_rel);
                    self.state.character.play_motion(&motion_path);
                } else {
                    tracing::warn!(
                        component = "CharacterWindow",
                        "No motion asset for the selected character; leaving rest pose"
                    );
                }
                cw.window.request_redraw();
                self.char_window = Some(cw);
            }
            Err(e) => {
                tracing::error!("Failed to create CharacterWindow: {e}");
                event_loop.exit();
                return;
            }
        }

        let visible = self.state.ui_bevy_state().0.settings_window_visible;
        if visible {
            self.create_ui_window(event_loop);
        }

        let chat_visible = self.state.chat_bevy_state().0.chat_window_visible;
        if chat_visible {
            self.create_chat_window(event_loop);
            self.state.reconcile_chat_history_if_needed();
        }
    }

    #[expect(
        clippy::similar_names,
        reason = "window_event matches winit callback parameter names"
    )]
    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        window_id: WindowId,
        event: WindowEvent,
    ) {
        let is_char = self
            .char_window
            .as_ref()
            .is_some_and(|w| w.window.id() == window_id);
        let is_ui = self
            .ui_window
            .as_ref()
            .is_some_and(|w| w.window.id() == window_id);
        let is_chat = self
            .chat_egui_window
            .as_ref()
            .is_some_and(|w| w.window.id() == window_id);
        let is_spotlight = self
            .spotlight_window
            .as_ref()
            .is_some_and(|w| w.window.id() == window_id);
        let is_caption = self
            .caption_window
            .as_ref()
            .is_some_and(|w| w.window.id() == window_id);

        // Track the Alt modifier across all windows so the Alt+Space
        // spotlight hotkey works no matter which window is focused.
        if let WindowEvent::ModifiersChanged(modifiers) = &event {
            self.alt_held = modifiers.state().alt_key();
        }
        #[cfg(target_os = "windows")]
        if let WindowEvent::ThemeChanged(theme) = &event {
            crate::theme::set_os_theme(match theme {
                winit::window::Theme::Light => crate::theme::ThemeMode::Light,
                winit::window::Theme::Dark => crate::theme::ThemeMode::Dark,
            });
        }

        // In-window Alt+Space fallback when no global grab is active
        // (Wayland, or Spotlight disabled). Works from any app window;
        // the grab consumes the key wherever it fires, so both paths
        // are never live at the same time.
        if let WindowEvent::KeyboardInput { .. } = &event
            && key_pressed(&event) == Some(NamedKey::Space)
            && self.alt_held
            && crate::hotkey::in_window_fallback_active(self.hotkey.is_registered())
        {
            self.toggle_spotlight(event_loop);
            return;
        }

        if is_ui {
            self.handle_ui_window_event(event_loop, event);
        } else if is_chat {
            self.handle_chat_window_event(event_loop, event);
        } else if is_spotlight {
            self.handle_spotlight_window_event(event_loop, event);
        } else if is_caption {
            self.handle_caption_window_event(event_loop, event);
        } else if is_char {
            self.handle_char_window_event(event_loop, event);
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if self.hotkey.consume_press() {
            self.toggle_spotlight(event_loop);
        }
        self.sync_theme();
        self.sync_runtime_to_bevy();
        self.state.app.update();
        self.handle_runtime_reconnect();
        if self.handle_exit(event_loop) {
            return;
        }
        self.run_debug_pipeline();
        self.render_per_frame(event_loop);
        self.set_frame_deadline(event_loop);
    }
}

impl Runtime {
    /// Push the persisted theme preference into the shared theme state,
    /// then propagate a resolved-theme change to every window's native
    /// decorations exactly once.
    fn sync_theme(&mut self) {
        crate::theme::set_preference(self.state.settings.theme());
        let preference = self.state.settings.theme();
        if self.native_theme_preference == Some(preference) {
            return;
        }
        self.native_theme_preference = Some(preference);
        let native_override = native_theme_override(preference);
        for window in [
            self.char_window.as_ref().map(|w| w.window.as_ref()),
            self.ui_window.as_ref().map(|w| w.window.as_ref()),
            self.chat_egui_window.as_ref().map(|w| w.window.as_ref()),
            self.spotlight_window.as_ref().map(|w| w.window.as_ref()),
            self.caption_window.as_ref().map(|w| w.window.as_ref()),
        ]
        .into_iter()
        .flatten()
        {
            window.set_theme(native_override);
        }
    }

    /// Push the per-frame runtime state into bevy resources before
    /// `app.update()` runs so the `should_render_debug_system` and the
    /// Linux-only `apply_linux_click_through_system` can read them.
    fn sync_runtime_to_bevy(&mut self) {
        let drag_active = self.state.character.drag.is_dragging();
        let debug_fps = self.state.settings.graphics().resolved().debug_fps;
        let transparent = self.transparent;
        self.state.app.world_mut().insert_resource(
            crate::system::platform::should_render_debug::DragActive(drag_active),
        );
        self.state.app.world_mut().insert_resource(
            crate::system::platform::should_render_debug::DebugFps(debug_fps),
        );
        self.state.app.world_mut().insert_resource(
            crate::system::platform::should_render_debug::TransparentWindow(transparent),
        );
        let expression_hold_secs = self
            .state
            .settings
            .config_section::<ene_mind::MindConfig>()
            .emotion
            .expression_hysteresis_seconds;
        if let Some(mut pipeline) =
            self.state
                .app
                .world_mut()
                .get_resource_mut::<crate::resource::emotion_pipeline::EmotionPipelineState>()
        {
            pipeline.expression_hold_secs = expression_hold_secs;
        }
        self.state.settings.flush_if_dirty();
        // Reconcile the OS grab with the persisted setting so disabling
        // Spotlight releases Alt+Space (and enabling re-registers it).
        if let Err(error) = self
            .hotkey
            .sync_enabled(self.state.settings.spotlight_enabled())
        {
            tracing::warn!(
                error = %error,
                "Failed to sync global hotkey registration"
            );
        }
    }

    fn handle_runtime_reconnect(&mut self) {
        if !self.state.take_reconnect_request() {
            return;
        }
        let handle = self
            .state
            .app
            .world()
            .resource::<crate::resource::tokio::TokioHandle>()
            .0
            .clone();
        match self.state.reconnect_runtime(&self.event_tx, &handle) {
            Ok(()) => {
                tracing::info!(component = "AiBridge", "Runtime reconnected");
            }
            Err(message) => {
                tracing::warn!(
                    component = "AiBridge",
                    error = %message,
                    "Runtime reconnect failed"
                );
            }
        }
    }

    fn handle_exit(&mut self, event_loop: &ActiveEventLoop) -> bool {
        let exit = self
            .state
            .app
            .world()
            .resource::<crate::resource::exit::ExitRequested>()
            .0;
        if !exit {
            return false;
        }
        self.persist_caption_settings();
        self.state.save();
        event_loop.exit();
        true
    }

    /// Run the per-frame debug pipeline (raycast, hover lookup) when the
    /// bevy `ShouldRenderDebug` resource permits. On non-Windows builds
    /// the cursor hit-test body itself no-ops.
    fn run_debug_pipeline(&mut self) {
        let Some(cw) = self.char_window.as_ref() else {
            return;
        };
        let should_update = self
            .state
            .app
            .world()
            .resource::<crate::system::platform::should_render_debug::ShouldRenderDebug>()
            .0;
        if !should_update {
            return;
        }
        let drag_is_dragging = self.state.character.drag.is_dragging();
        self.state.debug.last_raycast_hit = update_char_window_cursor_and_hittest(
            &mut self.state,
            &self.device_state,
            cw,
            self.transparent,
            drag_is_dragging,
            &mut self.last_cursor_physical,
        );
        #[cfg(target_os = "windows")]
        let hovered_name = if let Some(hit) = self.state.debug.last_raycast_hit {
            let colliders = self
                .state
                .character_physics_registration
                .as_ref()
                .map(|r| r.colliders.as_slice());
            if let Some(colliders) = colliders
                && let Some(idx) = colliders.iter().position(|&h| h == hit.collider)
            {
                self.state.character.get_active_bone_name(idx)
            } else {
                None
            }
        } else {
            None
        };
        #[cfg(not(target_os = "windows"))]
        let hovered_name = None;
        self.state.ui_bevy_state_mut().0.hovered_bone_name = hovered_name;
    }

    /// Render the character, chat, and settings windows in sequence.
    /// The character surface is fatal-error checked separately because a
    /// dead surface would otherwise loop silently; the chat / settings
    /// frame paths exit directly on their own fatal acquire errors.
    fn render_per_frame(&mut self, event_loop: &ActiveEventLoop) {
        self.render_char_frame();

        self.render_caption_frame(event_loop);
        self.render_spotlight_frame(event_loop);
        self.render_chat_frame(event_loop);
        self.render_settings_frame(event_loop);

        if self.char_surface_fatal {
            event_loop.exit();
        }
    }

    fn render_chat_frame(&mut self, event_loop: &ActiveEventLoop) {
        let chat_visible = self.state.chat_bevy_state().0.chat_window_visible;
        if chat_visible && self.chat_egui_window.is_none() {
            self.create_chat_window(event_loop);
            self.state.reconcile_chat_history_if_needed();
        } else if self.state.chat_bevy_state().0.needs_history_reconcile {
            self.state.reconcile_chat_history_if_needed();
        }

        let Some(cw) = self.chat_egui_window.as_mut() else {
            return;
        };
        if cw.window.is_visible() != Some(chat_visible) {
            cw.window.set_visible(chat_visible);
        }
        if !chat_visible {
            return;
        }
        cw.window.request_redraw();
        let Some(chat_entity) = self.state.chat_bevy_entity() else {
            return;
        };
        let bevy_world = self.state.app.world_mut();
        match cw.render_frame(
            &self.state.gpu.device,
            &self.state.gpu.queue,
            self.state.ai.as_ref(),
            bevy_world,
            chat_entity,
            &mut self.mic_handle,
        ) {
            Ok(()) => {}
            Err(e) => match e {
                AcquireError::Reconfigure => {
                    let size = cw.window.inner_size();
                    cw.reconfigure(&self.state.gpu.device, size);
                }
                AcquireError::Timeout => {}
                AcquireError::Fatal => event_loop.exit(),
            },
        }
    }

    fn render_spotlight_frame(&mut self, event_loop: &ActiveEventLoop) {
        let visible = self.state.ui_bevy_state().0.spotlight_visible
            && self.state.settings.spotlight_enabled();
        if !visible {
            self.spotlight_window = None;
            return;
        }
        if self.spotlight_window.is_none() {
            self.create_spotlight_window(event_loop);
        }
        let Some(w) = self.spotlight_window.as_mut() else {
            return;
        };
        w.window.request_redraw();
        let Some(ui_entity) = self.state.ui_bevy_entity() else {
            return;
        };
        let chat_entity = self.state.chat_bevy_entity().unwrap_or(Entity::PLACEHOLDER);
        let bevy_world = self.state.app.world_mut();
        match w.render_frame(
            &self.state.gpu.device,
            &self.state.gpu.queue,
            self.state.ai.as_ref(),
            bevy_world,
            ui_entity,
            chat_entity,
            &mut self.mic_handle,
        ) {
            Ok(()) => {}
            Err(e) => match e {
                AcquireError::Reconfigure => {
                    w.reconfigure(&self.state.gpu.device, w.window.inner_size());
                }
                AcquireError::Timeout => {}
                AcquireError::Fatal => event_loop.exit(),
            },
        }
    }

    fn render_caption_frame(&mut self, event_loop: &ActiveEventLoop) {
        let visible =
            self.state.ui_bevy_state().0.caption_visible && self.state.settings.caption_enabled();
        if !visible {
            if self.caption_window.is_some() {
                self.persist_caption_settings();
                self.caption_window = None;
            }
            return;
        }
        if self.caption_window.is_none() {
            self.create_caption_window(event_loop);
        }
        let Some(w) = self.caption_window.as_mut() else {
            return;
        };
        w.window.request_redraw();
        let Some(ui_entity) = self.state.ui_bevy_entity() else {
            return;
        };
        let bevy_world = self.state.app.world_mut();
        match w.render_frame(
            &self.state.gpu.device,
            &self.state.gpu.queue,
            bevy_world,
            ui_entity,
        ) {
            Ok(()) => {}
            Err(e) => match e {
                AcquireError::Reconfigure => {
                    w.reconfigure(&self.state.gpu.device, w.window.inner_size());
                }
                AcquireError::Timeout => {}
                AcquireError::Fatal => event_loop.exit(),
            },
        }
    }

    /// Persist the caption overlay's position / pin state so the next
    /// launch restores it. Called when the overlay hides or the app
    /// exits; the values live in `UiState` while the window is open.
    fn persist_caption_settings(&mut self) {
        let (position, pinned) = {
            let ui_state = self.state.ui_bevy_state();
            (ui_state.0.caption_position, ui_state.0.caption_pinned)
        };
        if position.is_none() && pinned.is_none() {
            return;
        }
        let mut desktop = self.state.settings.config_section::<DesktopSection>();
        if position.is_some() {
            desktop.caption_position = position;
        }
        if pinned.is_some() {
            desktop.caption_pinned = pinned;
        }
        self.state.settings.set_config_section(&desktop);
        self.state.settings.mark_dirty();
    }

    fn render_settings_frame(&mut self, event_loop: &ActiveEventLoop) {
        // A discard decision from the editor dialog clears the unsaved flag
        // while keeping `app_exit_requested` set; complete the exit here.
        let exit_after_discard = {
            let ui_state = self.state.ui_bevy_state();
            ui_state.0.app_exit_requested && !ui_state.0.editor_has_unsaved_changes()
        };
        if exit_after_discard {
            self.state.save();
            event_loop.exit();
            return;
        }

        // A discard decision from the editor dialog clears the unsaved flag
        // while keeping `close_requested` set; complete the close here.
        let close_after_discard = {
            let ui_state = self.state.ui_bevy_state();
            ui_state.0.character_editor_close_requested && !ui_state.0.editor_has_unsaved_changes()
        };
        if close_after_discard {
            self.hide_settings_window();
            return;
        }

        // A discard decision from the settings close dialog clears the draft
        // while keeping `settings_close_requested` set; complete the close.
        let close_settings_after_discard = {
            let ui_state = self.state.ui_bevy_state();
            ui_state.0.settings_close_requested
                && self
                    .ui_window
                    .as_ref()
                    .is_none_or(|uw| !uw.settings_ui.draft.is_dirty())
        };
        if close_settings_after_discard {
            self.hide_settings_window();
            return;
        }

        let visible = self.state.ui_bevy_state().0.settings_window_visible;
        if visible && self.ui_window.is_none() {
            // A stale close request from a previous session must not hide a
            // freshly opened window; it was consumed by the discard flow.
            self.state
                .ui_bevy_state_mut()
                .0
                .character_editor_close_requested = false;
            self.create_ui_window(event_loop);
            if let Some(uw) = self.ui_window.as_mut() {
                let ui_state_snapshot = self.state.ui_bevy_state().0.clone();
                uw.settings_ui
                    .sync_from_settings(&self.state.settings, &ui_state_snapshot);
            }
        }

        if let Some(uw) = self.ui_window.as_mut() {
            let visible = self.state.ui_bevy_state().0.settings_window_visible;
            if uw.window.is_visible() != Some(visible) {
                uw.window.set_visible(visible);
            }
            if !visible {
                return;
            }
            uw.window.request_redraw();
            let Some(ui_entity) = self.state.ui_bevy_entity() else {
                return;
            };
            let bevy_world = self.state.app.world_mut();
            let now_secs = self.emotion_clock.elapsed().as_secs_f64();
            match uw.render_frame(
                &self.state.gpu.device,
                &self.state.gpu.queue,
                &mut self.state.settings,
                self.state.ai.as_ref(),
                bevy_world,
                ui_entity,
                now_secs,
            ) {
                Ok(()) => {}
                Err(e) => match e {
                    AcquireError::Reconfigure => {
                        let size = uw.window.inner_size();
                        uw.reconfigure(&self.state.gpu.device, size);
                    }
                    AcquireError::Timeout => {}
                    AcquireError::Fatal => {
                        event_loop.exit();
                    }
                },
            }

            // Drain the legacy SettingsUi::emotion_queue (filled by
            // the manual expression buttons) into the Bevy
            // UiEmotionQueue component so that apply_emotions_system
            // can forward them to EmotionPipelineState::pending.
            if !uw.settings_ui.emotion_queue.commands.is_empty() {
                use crate::component::ui::UiEmotionQueue;
                if let Some(ui_entity2) = self.state.ui_bevy_entity() {
                    let bevy_world2 = self.state.app.world_mut();
                    if let Some(mut eq) = bevy_world2.get_mut::<UiEmotionQueue>(ui_entity2) {
                        while let Some(cmd) = uw.settings_ui.emotion_queue.commands.pop_front() {
                            eq.0.commands.push_back(cmd);
                        }
                    }
                }
            }
        }
    }

    /// Schedule the next winit wake-up based on
    /// `settings.graphics.target_fps`. `0` means "poll
    /// continuously".
    fn set_frame_deadline(&mut self, event_loop: &ActiveEventLoop) {
        let target_fps = self.state.settings.graphics().resolved().target_fps;
        if target_fps == 0 {
            event_loop.set_control_flow(ControlFlow::Poll);
            return;
        }
        let frame_interval = Duration::from_secs_f64(f64::from(target_fps).recip());
        let last = self.last_frame_instant.unwrap_or_else(Instant::now);
        let next_deadline = last + frame_interval;
        event_loop.set_control_flow(ControlFlow::WaitUntil(next_deadline));
    }
}

impl Runtime {
    fn handle_char_window_event(&mut self, event_loop: &ActiveEventLoop, event: WindowEvent) {
        let Some(cw) = self.char_window.as_mut() else {
            return;
        };
        match event {
            WindowEvent::CloseRequested => {
                self.request_app_exit(event_loop);
            }
            WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. } => {
                let AppState {
                    ref mut character,
                    ref gpu,
                    ..
                } = self.state;
                let new_size = cw.window.inner_size();
                cw.reconfigure(&gpu.device, new_size);
                character.resize(&gpu.device, (new_size.width, new_size.height));
                character.resize_post_processor(
                    &gpu.device,
                    &gpu.queue,
                    (new_size.width, new_size.height),
                );
                #[cfg(target_os = "linux")]
                {
                    use crate::resource::platform_state::resources::MaskCapture;
                    let world = self.state.app.world();
                    if let Some(mask) = world.resource::<MaskCapture>().0.as_ref() {
                        let downsample = self
                            .state
                            .settings
                            .graphics()
                            .resolved()
                            .mask_render_downsample;
                        let mut guard = mask.lock();
                        let _ =
                            guard.resize(&gpu.device, new_size.width, new_size.height, downsample);
                    }
                }
                cw.window.request_redraw();
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.last_cursor_physical = Some(position);
                cw.window.request_redraw();

                let eye = self.state.character.camera_eye();
                let target = self.state.character.camera_target();
                let cursor_world_2d = cursor_world_2d_for_char_window(cw, eye, target, position);
                let AppState {
                    ref mut character,
                    ref mut settings,
                    ..
                } = self.state;
                if let Some(delta) =
                    crate::character::drag::tick(&mut character.drag, cursor_world_2d)
                {
                    settings.character_state.character_position += delta;
                    let height = cw.window.inner_size().height as f32;
                    if height > 0.0 {
                        let pixel_size = ene_vrm::camera::VIEWPORT_HEIGHT / height;
                        settings.character_state.character_position.x =
                            (settings.character_state.character_position.x / pixel_size).round()
                                * pixel_size;
                        settings.character_state.character_position.y =
                            (settings.character_state.character_position.y / pixel_size).round()
                                * pixel_size;
                    }
                }
            }
            WindowEvent::MouseInput {
                state: btn_state,
                button: MouseButton::Left,
                ..
            } => {
                use crate::character::drag::{DragAction, DragButtonEvent};
                let Some(cursor_phys) = self.last_cursor_physical else {
                    return;
                };
                let eye = self.state.character.camera_eye();
                let target = self.state.character.camera_target();
                let AppState {
                    ref mut character, ..
                } = self.state;
                let event = match btn_state {
                    ElementState::Pressed => DragButtonEvent::Pressed,
                    ElementState::Released => DragButtonEvent::Released,
                };
                let cursor_world_2d = cursor_world_2d_for_char_window(cw, eye, target, cursor_phys);
                #[cfg(target_os = "windows")]
                let cursor_over = {
                    let app = &self.state.app;
                    let physics_world = app
                        .world()
                        .resource::<crate::resource::physics::PhysicsWorldResource>();
                    let scale = cw.window.scale_factor();
                    let logical_size = cw.window.inner_size().to_logical::<f64>(scale);
                    let logical = cursor_phys.to_logical::<f64>(scale);
                    let ndc_x = (logical.x / logical_size.width.max(1.0)) * 2.0 - 1.0;
                    let ndc_y = -((logical.y / logical_size.height.max(1.0)) * 2.0 - 1.0);
                    let aspect = (logical_size.width / logical_size.height.max(0.0001)) as f32;
                    let half_h = ene_vrm::camera::VIEWPORT_HEIGHT * 0.5;
                    let half_w = half_h * aspect;
                    let view = glam::camera::rh::view::look_at_mat4(
                        eye.into(),
                        target.into(),
                        ene_vrm::camera::DEFAULT_UP.into(),
                    );
                    let view_pos =
                        glam::Vec3::new(ndc_x as f32 * half_w, ndc_y as f32 * half_h, 0.0);
                    let world_3d = view.inverse().transform_point3(view_pos);
                    let ray_dir =
                        glam::Vec3::new(target[0] - eye[0], target[1] - eye[1], target[2] - eye[2]);
                    physics_world
                        .world
                        .cast_ray(world_3d, ray_dir, 100.0)
                        .is_some()
                };
                #[cfg(not(target_os = "windows"))]
                let cursor_over = true;
                let action = crate::character::drag::on_press_or_release(
                    &mut character.drag,
                    event,
                    cursor_world_2d,
                    cursor_over,
                );
                if matches!(action, DragAction::Ended) {
                    let AppState {
                        ref mut settings, ..
                    } = self.state;
                    settings.mark_dirty();
                }
            }
            WindowEvent::KeyboardInput { .. } => {
                if let Some(named) = key_pressed(&event) {
                    if matches!(named, NamedKey::Space) {
                        self.transparent = !self.transparent;
                        cw.window.set_decorations(!self.transparent);
                        cw.window.set_transparent(self.transparent);
                        cw.window.request_redraw();
                    } else if matches!(named, NamedKey::Escape) {
                        self.request_app_exit(event_loop);
                    } else if matches!(named, NamedKey::F1) {
                        let visible = self.state.ui_bevy_state().0.settings_window_visible;
                        if visible {
                            self.hide_settings_window();
                        } else {
                            self.show_settings_window(event_loop);
                        }
                    } else if key_code_pressed(&event) == Some(winit::keyboard::KeyCode::F2) {
                        let visible = self.state.chat_bevy_state().0.chat_window_visible;
                        if visible {
                            self.hide_chat_window();
                        } else {
                            self.show_chat_window();
                            if self.chat_egui_window.is_none() {
                                self.create_chat_window(event_loop);
                            }
                            self.state.reconcile_chat_history_if_needed();
                            if let Some(cw) = self.chat_egui_window.as_ref() {
                                cw.window.request_redraw();
                            }
                        }
                    } else if key_code_pressed(&event) == Some(winit::keyboard::KeyCode::F3) {
                        let mut ui_state = self.state.ui_bevy_state_mut();
                        ui_state.0.show_collider_debug = !ui_state.0.show_collider_debug;
                        cw.window.request_redraw();
                    } else if key_code_pressed(&event) == Some(winit::keyboard::KeyCode::F8) {
                        #[cfg(target_os = "linux")]
                        {
                            use crate::resource::platform_state::resources::LayerShellFreeze;
                            let new_val = {
                                let world = self.state.app.world_mut();
                                let mut freeze = world.resource_mut::<LayerShellFreeze>();
                                freeze.0 = !freeze.0;
                                freeze.0
                            };
                            tracing::info!(
                                target: "ene.linux.layer_shell",
                                freeze = new_val,
                                "char window freeze toggled"
                            );
                        }
                        cw.window.request_redraw();
                    } else if key_code_pressed(&event) == Some(winit::keyboard::KeyCode::F9) {
                        {
                            let next_val = {
                                let mut ui_state = self.state.ui_bevy_state_mut();
                                ui_state.0.show_input_region_debug =
                                    !ui_state.0.show_input_region_debug;
                                ui_state.0.show_input_region_debug
                            };
                            tracing::info!(
                                target: "ene.linux.input_region",
                                show = next_val,
                                "input-region debug overlay toggled"
                            );
                        }
                        cw.window.request_redraw();
                    } else {
                        let visible = self.state.ui_bevy_state().0.settings_window_visible;
                        if visible
                            && let Some(action) = char_settings_hotkey_from_event(
                                &event,
                                cw_char_window_has_focus(cw),
                            )
                        {
                            self.dispatch_settings_action(action);
                        }
                    }
                }
            }
            _ => {}
        }
    }

    /// Render the character window directly from `about_to_wait`.
    ///
    /// A fatal surface acquire error sets `char_surface_fatal` so the
    /// caller can exit the event loop; `Reconfigure` and `Timeout` are
    /// handled inline.
    fn render_char_frame(&mut self) {
        let Some(cw) = self.char_window.as_mut() else {
            return;
        };
        let transparent = self.transparent;
        let (show_collider_debug, show_mask_gizmo, show_input_region_debug) = {
            let ui_state = self.state.ui_bevy_state();
            (
                ui_state.0.show_collider_debug,
                ui_state.0.debug_overlay_visible,
                ui_state.0.show_input_region_debug,
            )
        };
        let last_raycast_hit = self.state.debug.last_raycast_hit;
        let motion_playing = {
            use crate::component::ui::UiAnimation;
            self.state
                .ui_bevy_entity()
                .and_then(|entity| {
                    self.state
                        .app
                        .world()
                        .get::<UiAnimation>(entity)
                        .map(|a| a.0.playing)
                })
                .or_else(|| {
                    self.ui_window
                        .as_ref()
                        .map(|uw| uw.settings_ui.animation.playing)
                })
                .unwrap_or(true)
        };
        let AppState {
            ref mut character,
            ref mut debug,
            ref mut character_physics_registration,
            ref gpu,
            ref mut settings,
            ref mut app,
            ..
        } = self.state;
        let debug_renderer = &mut debug.debug_renderer;
        let (device, queue) = (&gpu.device, &gpu.queue);

        if settings.character_state.needs_respawn {
            settings.character_state.needs_respawn = false;

            if let Some(new_vrm_rel) = settings.current_character() {
                let new_vrm_path = settings.assets_dir.join(new_vrm_rel);
                let current_vrm_path = character.default_vrm_path();

                if Some(new_vrm_path.as_path()) != current_vrm_path {
                    character.set_default_vrm(new_vrm_path);

                    let format = cw.config.format;
                    #[cfg(target_os = "linux")]
                    let mask_format =
                        Some(crate::platform::wayland_mask_capture::MASK_TARGET_FORMAT);
                    #[cfg(not(target_os = "linux"))]
                    let mask_format = None;

                    character.init(&gpu.device, &gpu.queue, format, mask_format);

                    character.resize(
                        &gpu.device,
                        (cw.window.inner_size().width, cw.window.inner_size().height),
                    );

                    #[cfg(target_os = "windows")]
                    {
                        *character_physics_registration = None;
                        let actual_scale = character.auto_fit_scale(CHARACTER_AUTO_FIT_MARGIN)
                            * settings.character_state.model_scale;
                        let specs = character.build_character_bone_specs(actual_scale);
                        if !specs.is_empty() {
                            let mut physics_res = app
                                .world_mut()
                                .resource_mut::<crate::resource::physics::PhysicsWorldResource>(
                            );
                            let registration =
                                physics_res.world.register_character_colliders(&specs);
                            *character_physics_registration = Some(registration);
                        }
                    }
                }
            } else {
                tracing::warn!(
                    component = "CharacterRespawn",
                    "Selected character has no VRM; skipping model reload"
                );
            }

            if let Some(motion_rel) = settings.current_motion() {
                let motion_path = settings.assets_dir.join(motion_rel);
                character.play_motion(&motion_path);
            } else {
                tracing::warn!(
                    component = "CharacterRespawn",
                    "Selected character has no motion; leaving rest pose"
                );
            }
        }

        // Tick the emotion pipeline early; apply morph weights after
        // look-at so gaze morphs are composed first and emotion
        // overrides win on conflicting targets.
        let applied_emotion = {
            let world = app.world_mut();
            let pipeline =
                world.get_resource_mut::<crate::resource::emotion_pipeline::EmotionPipelineState>();
            if let Some(mut pipeline) = pipeline {
                let now_secs = self.emotion_clock.elapsed().as_secs_f64();
                Some(crate::resource::emotion_pipeline::tick_emotions(
                    &mut pipeline,
                    now_secs,
                ))
            } else {
                None
            }
        };

        let cs = &settings.character_state;
        let actual_scale = character.auto_fit_scale(CHARACTER_AUTO_FIT_MARGIN) * cs.model_scale;
        character.update_camera_target(actual_scale);
        let model_uniform = ene_vrm::ModelUniform::from_mat4(
            character.model_matrix(cs.character_position, actual_scale),
        );

        let now = Instant::now();
        let dt_secs = self
            .last_frame_instant
            .map_or(1.0 / 60.0, |t| now.duration_since(t).as_secs_f32())
            .clamp(0.0, 0.1);
        self.last_frame_instant = Some(now);

        {
            let world = app.world_mut();
            let motion_layer =
                world.get_resource_mut::<crate::resource::motion_layer::MotionLayerState>();
            if let Some(mut motion_layer) = motion_layer {
                motion_layer.tick(dt_secs);
                let frame = motion_layer.compose();

                if let Some(motion_name) = frame.active_motions.first() {
                    let should_switch =
                        character.active_motion_name() != Some(motion_name.as_str());
                    if should_switch {
                        let resolved = settings.current_entry().and_then(|entry| {
                            entry
                                .motion_names
                                .iter()
                                .position(|n| n == motion_name.as_str())
                                .and_then(|idx| entry.motion_paths.get(idx))
                                .map(|rel| settings.assets_dir.join(rel))
                        });
                        if let Some(path) = resolved {
                            character.play_motion(&path);
                        } else if let Err(e) = character.play_motion_by_name(motion_name) {
                            tracing::warn!(
                                component = "MotionLayer",
                                motion = %motion_name,
                                error = %e,
                                "Failed to load motion clip"
                            );
                        }
                    }
                }
            }
        }

        // Beat sync: drain pulses relayed through the runtime chat bus and
        // drive the avatar's procedural sway + locomotion speed sync.
        {
            // A capture thread that died (device unplug, stream error) must
            // not leave the avatar waiting for pulses that will never come;
            // the toggle UI reads the same liveness flag.
            #[cfg(feature = "voice")]
            let beat_running = app
                .world()
                .get_resource::<crate::resource::beat_sync::BeatSyncRuntime>()
                .is_some_and(crate::resource::beat_sync::BeatSyncRuntime::is_running);
            #[cfg(not(feature = "voice"))]
            let beat_running = false;
            let world = app.world_mut();
            let beat_state = world.get_resource_mut::<crate::resource::beat_sync::BeatSyncState>();
            if let Some(mut beat_state) = beat_state {
                if beat_state.is_enabled() && !beat_running {
                    beat_state.set_enabled(false);
                }
                character.set_beat_sync_enabled(beat_state.is_enabled());
                for pulse in beat_state.drain_pulses() {
                    character.beat_pulse(pulse.bpm, pulse.intensity);
                }
            }
        }

        character.set_motion_player_playing(motion_playing);

        if let Some(palette) = character.update_motion(dt_secs) {
            character.update_skin_palette_gpu(queue, &palette);
        }

        #[cfg(target_os = "windows")]
        if let Some(reg) = &character_physics_registration {
            let poses = character.current_bone_poses();
            let mut physics_res = app
                .world_mut()
                .resource_mut::<crate::resource::physics::PhysicsWorldResource>();
            physics_res.world.update_character_bone_positions(
                reg,
                &poses,
                cs.character_position,
                actual_scale,
            );
        }

        if let Some(cursor) = self.last_cursor_physical {
            let viewport: (u32, u32) =
                (cw.window.inner_size().width, cw.window.inner_size().height);
            let _smoothed = character.update_look_at(
                glam::Vec2::new(cursor.x as f32, cursor.y as f32),
                viewport,
                cs.character_position,
                actual_scale,
                cs.look_at_strength,
                dt_secs,
            );
        }

        if let Some(applied) = applied_emotion
            && !applied.name.is_empty()
            && let Some(model) = character.model_mut()
        {
            let expressions_meta = model.expressions_meta.clone();
            let layer = model.expressions_mut();
            tracing::debug!(
                expression = %applied.name,
                weight = applied.weight,
                "apply emotion"
            );
            let names = match applied.name.as_str() {
                "happy" => vec!["happy", "joy"],
                "sad" => vec!["sad", "sorrow"],
                "relaxed" => vec!["relaxed"],
                other => vec![other],
            };
            for preset in [
                "neutral",
                "happy",
                "sad",
                "angry",
                "relaxed",
                "surprised",
                "joy",
                "sorrow",
                "fun",
            ] {
                let name = ene_vrm::ExpressionName::new(preset.to_string());
                layer.set_expression(&name, 0.0);
            }
            for name_str in names {
                let name = ene_vrm::ExpressionName::new(name_str.to_string());
                layer.set_expression(&name, applied.weight);
            }
            layer.apply_overrides(&expressions_meta);
        }

        // Viseme lip-sync: while TTS audio is playing, read the smoothed
        // mouth-shape weights from the shared viseme driver and apply them
        // on top of the current expression, then re-run overrides so the
        // morph targets uploaded to the GPU reflect the viseme blend.
        #[cfg(feature = "voice")]
        {
            let tts_playing = app
                .world()
                .get_resource::<crate::audio::AudioState>()
                .is_some_and(crate::audio::AudioState::is_tts_playing);
            // Consume queued PCM up to the current playback position so the
            // analyzer only sees samples that are actually playing.
            if tts_playing
                && let Some(viseme) = app.world().get_resource::<crate::audio::VisemeState>()
            {
                viseme.advance();
            }
            if tts_playing
                && let Some(viseme) = app.world().get_resource::<crate::audio::VisemeState>()
                && let Some(weights) = viseme.analyze_weights()
                && let Some(model) = character.model_mut()
            {
                let expressions_meta = model.expressions_meta.clone();
                let layer = model.expressions_mut();
                layer.apply_viseme_weights(&weights);
                layer.apply_overrides(&expressions_meta);
            } else if !tts_playing
                && self.prev_tts_playing
                && let Some(model) = character.model_mut()
            {
                // On the frame where `tts_playing` transitions from
                // true to false, apply zeroed viseme weights so the mouth
                // shape resets instead of holding the last phoneme.
                let expressions_meta = model.expressions_meta.clone();
                let layer = model.expressions_mut();
                layer.apply_viseme_weights(&ene_vrm::viseme::VisemeWeights::default());
                layer.apply_overrides(&expressions_meta);
            }
            self.prev_tts_playing = tts_playing;
        }

        let result = cw.with_surface_view(|view| {
            let swapchain_size = (cw.config.width, cw.config.height);
            let aa_mode = settings.graphics().resolved().antialiasing_mode;
            character.render(
                device,
                queue,
                view,
                transparent,
                &model_uniform,
                swapchain_size,
                cw.config.format,
                aa_mode,
            );
            let show_any_debug = show_collider_debug || show_mask_gizmo || show_input_region_debug;
            if show_any_debug && let Some(depth_view) = character.depth_view() {
                let camera_eye = glam::Vec3::from(character.camera_eye());
                let camera_target = glam::Vec3::from(character.camera_target());
                let camera_distance = (camera_eye - camera_target).length();
                #[cfg_attr(
                    target_os = "windows",
                    expect(unused_variables, reason = "documented exception for this lint")
                )]
                let view_z = -camera_distance;
                let cam_view = glam::camera::rh::view::look_at_mat4(
                    camera_eye,
                    camera_target,
                    ene_vrm::camera::DEFAULT_UP.into(),
                );
                let mut lines = Vec::new();
                if show_collider_debug {
                    let (hit_collider, hit_point) = match last_raycast_hit {
                        Some(h) => (Some(h.collider), Some(h.point)),
                        None => (None, None),
                    };
                    if let Some(colliders) = character_physics_registration
                        .as_ref()
                        .map(|r| r.colliders.as_slice())
                    {
                        let app = &self.state.app;
                        let physics_world =
                            app.world()
                                .resource::<crate::resource::physics::PhysicsWorldResource>();
                        crate::raycast_debug::build_collider_lines(
                            &mut lines,
                            &physics_world.world,
                            colliders,
                            hit_collider,
                            hit_point,
                            true,
                        );
                    }
                    if let Some(model) = character.model() {
                        let actual_scale =
                            character.auto_fit_scale(CHARACTER_AUTO_FIT_MARGIN) * cs.model_scale;
                        crate::skeleton_debug::build_skeleton_lines(
                            &mut lines,
                            model,
                            cs.character_position,
                            actual_scale,
                        );
                    }
                }
                if show_mask_gizmo {
                    #[cfg(target_os = "linux")]
                    {
                        use crate::resource::platform_state::resources::MaskCapture;
                        let world = self.state.app.world();
                        if let Some(mask) = world.resource::<MaskCapture>().0.as_ref() {
                            crate::mask_gizmo::build_mask_rect_lines(
                                &mut lines,
                                mask,
                                cw.config.width,
                                cw.config.height,
                                cam_view,
                                view_z,
                            );
                        }
                    }
                }

                if show_input_region_debug {
                    #[cfg(target_os = "linux")]
                    {
                        use crate::resource::platform_state::resources::{
                            LastAppliedInputRects, LastInputSource,
                        };
                        let world = self.state.app.world();
                        crate::input_region_debug::build_input_region_debug_lines(
                            &mut lines,
                            &world.resource::<LastAppliedInputRects>().0,
                            world.resource::<LastInputSource>().0,
                            cw.config.width,
                            cw.config.height,
                            cam_view,
                            view_z,
                        );
                    }
                }

                if !lines.is_empty() {
                    if debug_renderer.is_none() {
                        *debug_renderer =
                            Some(ene_vrm::DebugRenderer::new(device, cw.config.format));
                    }
                    if let Some(debug) = debug_renderer.as_mut() {
                        for line in &lines {
                            debug.push_line(*line);
                        }
                        // `camera_uniform_dbg` returns `Option` for API symmetry with
                        // the debug pipeline, but `Camera::uniform` is infallible by
                        // construction (see `ene_vrm::camera::OrthographicCamera::uniform`).
                        #[expect(
                            clippy::expect_used,
                            reason = "Camera::uniform is infallible by construction"
                        )]
                        let camera_uniform = character
                            .camera_uniform_dbg()
                            .expect("orthographic camera uniform is infallible");
                        let mut encoder =
                            device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                                label: Some("debug.encoder"),
                            });
                        debug.render(
                            device,
                            queue,
                            &mut encoder,
                            view,
                            depth_view,
                            &camera_uniform,
                        );
                        queue.submit(std::iter::once(encoder.finish()));
                    }
                }
            }
        });

        #[cfg(target_os = "linux")]
        {
            use crate::resource::platform_state::resources::{MaskCapture, MaskReadbackWorkerRes};
            let world = self.state.app.world();
            if let Some(mask) = world.resource::<MaskCapture>().0.as_ref() {
                let mut mask_guard = mask.lock();
                let downsample = settings.graphics().resolved().mask_render_downsample;
                let _ = mask_guard.resize(device, cw.config.width, cw.config.height, downsample);
                let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("mask.encoder"),
                });
                character.render_mask(
                    queue,
                    &mut encoder,
                    mask_guard.target_view(),
                    &model_uniform,
                );
                mask_guard.encode_readback(&mut encoder);
                // Keep mask_guard alive (locked) through queue.submit so the
                // background readback thread cannot call readback_slice() and
                // map the buffer between encode_readback and submit, which would
                // cause a wgpu validation error ("Buffer still mapped").
                queue.submit(std::iter::once(encoder.finish()));
                drop(mask_guard);
                if let Some(worker) = world.resource::<MaskReadbackWorkerRes>().0.as_ref() {
                    worker.request_readback();
                }
            }
        }
        match result {
            Err(AcquireError::Reconfigure) => {
                tracing::warn!("Character Surface acquire Outdated/Lost; reconfiguring");
                cw.reconfigure(device, cw.window.inner_size());
            }
            Ok(()) | Err(AcquireError::Timeout) => {}
            Err(AcquireError::Fatal) => {
                tracing::error!("Character Surface acquire failed fatally; exiting");
                self.char_surface_fatal = true;
            }
        }
    }

    fn handle_ui_window_event(&mut self, _event_loop: &ActiveEventLoop, event: WindowEvent) {
        // `if let Some` keeps a misrouted window id a silent no-op instead
        // of a panic that kills the event loop.
        let Some(uw) = self.ui_window.as_mut() else {
            return;
        };

        if event == WindowEvent::CloseRequested {
            self.hide_settings_window();
            return;
        }

        let window = Arc::clone(&uw.window);
        let response = uw.egui_state.on_window_event(&window, &event);
        if response.repaint {
            uw.window.request_redraw();
        }
        if response.consumed {
            return;
        }
        match event {
            WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. } => {
                let size = uw.window.inner_size();
                uw.reconfigure(&self.state.gpu.device, size);
                uw.window.request_redraw();
            }
            WindowEvent::KeyboardInput { .. } => {
                let pressed = key_pressed(&event);
                if matches!(pressed, Some(NamedKey::Escape | NamedKey::F1)) {
                    self.hide_settings_window();
                }
            }
            _ => {}
        }
    }

    fn handle_chat_window_event(&mut self, _event_loop: &ActiveEventLoop, event: WindowEvent) {
        let Some(cw) = self.chat_egui_window.as_mut() else {
            return;
        };

        if event == WindowEvent::CloseRequested {
            self.hide_chat_window();
            return;
        }

        let window = Arc::clone(&cw.window);
        let response = cw.egui_state.on_window_event(&window, &event);
        if response.repaint {
            cw.window.request_redraw();
        }
        if response.consumed {
            return;
        }
        match event {
            WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. } => {
                let size = cw.window.inner_size();
                cw.reconfigure(&self.state.gpu.device, size);
                cw.window.request_redraw();
            }
            WindowEvent::KeyboardInput { .. } => {
                if key_pressed(&event) == Some(NamedKey::Escape) {
                    self.hide_chat_window();
                }
            }
            _ => {}
        }
    }

    fn handle_spotlight_window_event(&mut self, _event_loop: &ActiveEventLoop, event: WindowEvent) {
        let Some(w) = self.spotlight_window.as_mut() else {
            return;
        };

        if event == WindowEvent::CloseRequested {
            self.state.ui_bevy_state_mut().0.spotlight_visible = false;
            return;
        }

        let response = w.egui_state.on_window_event(&w.window, &event);
        if response.repaint {
            w.window.request_redraw();
        }
        if response.consumed {
            return;
        }
        match event {
            WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. } => {
                w.reconfigure(&self.state.gpu.device, w.window.inner_size());
                w.window.request_redraw();
            }
            _ => {}
        }
    }

    fn handle_caption_window_event(&mut self, _event_loop: &ActiveEventLoop, event: WindowEvent) {
        let Some(w) = self.caption_window.as_mut() else {
            return;
        };

        if event == WindowEvent::CloseRequested {
            self.state.ui_bevy_state_mut().0.caption_visible = false;
            return;
        }

        let response = w.egui_state.on_window_event(&w.window, &event);
        if response.repaint {
            w.window.request_redraw();
        }
        if response.consumed {
            return;
        }
        match event {
            WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. } => {
                w.reconfigure(&self.state.gpu.device, w.window.inner_size());
                w.window.request_redraw();
            }
            _ => {}
        }
    }

    fn dispatch_settings_action(&mut self, action: crate::settings_ui::widgets::SettingsAction) {
        let Some(uw) = self.ui_window.as_mut() else {
            return;
        };
        if uw.settings_ui.current_page != PageKind::Character {
            return;
        }
        let AppState {
            ref mut settings,
            ref ai,
            ref mut app,
            ..
        } = self.state;
        let Some(ai) = ai.as_ref() else {
            return;
        };
        let Some(ui_entity) = app
            .world_mut()
            .query_filtered::<
                bevy_ecs::entity::Entity,
                bevy_ecs::prelude::With<crate::component::ui::UiWindow>,
            >()
            .iter(app.world())
            .next()
        else {
            return;
        };
        // Write the per-action message so the dispatcher system in
        // `system::ui_dispatcher` can observe / drain it on the next frame.
        // The actual mutation still happens through `apply_action`
        // because `CharacterSettings` is not a bevy `Resource` yet.
        app.world_mut()
            .write_message(crate::event::ui_action::SettingsActionEvent {
                action: action.clone(),
            });
        let bevy_world = app.world_mut();
        let now_secs = uw.settings_ui.started_at.elapsed().as_secs_f64();
        crate::settings_ui::widgets::apply_action(
            action,
            settings,
            &mut uw.settings_ui.animation,
            ai,
            bevy_world,
            ui_entity,
            Some(&mut uw.settings_ui.emotion_queue),
            now_secs,
        );
    }
}

fn cw_char_window_has_focus(cw: &CharacterWindow) -> bool {
    cw.window.has_focus()
}

/// Compute the cursor's 2D world position for the drag hit-test.
fn cursor_world_2d_for_char_window(
    cw: &CharacterWindow,
    eye: [f32; 3],
    target: [f32; 3],
    position: winit::dpi::PhysicalPosition<f64>,
) -> Option<glam::Vec2> {
    use crate::character::drag::cursor_logical_to_world_2d;
    let size = cw.window.inner_size();
    let scale = cw.window.scale_factor();
    let logical = position.to_logical::<f64>(scale);
    let logical_size = size.to_logical::<f64>(scale);
    let viewport = (
        logical_size.width.max(1.0) as u32,
        logical_size.height.max(1.0) as u32,
    );
    let eye = eye.into();
    let target = target.into();
    let up = ene_vrm::camera::DEFAULT_UP.into();
    cursor_logical_to_world_2d(
        glam::Vec2::new(logical.x as f32, logical.y as f32),
        viewport,
        eye,
        target,
        up,
    )
}

/// Per-frame click-through update for the character window.
fn update_char_window_cursor_and_hittest(
    state: &mut AppState,
    device_state: &device_query::DeviceState,
    cw: &CharacterWindow,
    transparent: bool,
    drag_is_dragging: bool,
    last_cursor: &mut Option<winit::dpi::PhysicalPosition<f64>>,
) -> Option<crate::physics::RayHit> {
    let mouse = device_state.get_mouse();
    let (gx, gy) = (mouse.coords.0, mouse.coords.1);

    let mut local_cursor = None;
    let hit = if let Ok(outer) = cw.window.outer_position() {
        let local_physical_x = f64::from(gx) - f64::from(outer.x);
        let local_physical_y = f64::from(gy) - f64::from(outer.y);

        local_cursor = Some(winit::dpi::PhysicalPosition::new(
            local_physical_x,
            local_physical_y,
        ));

        #[cfg(target_os = "windows")]
        let scale = cw.window.scale_factor();
        #[cfg(target_os = "windows")]
        let logical_x = local_physical_x / scale;
        #[cfg(target_os = "windows")]
        let logical_y = local_physical_y / scale;
        #[cfg(target_os = "windows")]
        let inner = cw.window.inner_size();
        #[cfg(target_os = "windows")]
        let logical_w = (inner.width as f64 / scale).max(1.0);
        #[cfg(target_os = "windows")]
        let logical_h = (inner.height as f64 / scale).max(1.0);
        #[cfg(target_os = "windows")]
        let ndc_x = (logical_x / logical_w) * 2.0 - 1.0;
        #[cfg(target_os = "windows")]
        let ndc_y = -((logical_y / logical_h) * 2.0 - 1.0);

        #[cfg(target_os = "windows")]
        let inside_window = ndc_x.abs() <= 1.0 && ndc_y.abs() <= 1.0;

        #[cfg(target_os = "windows")]
        let hit = if inside_window {
            let eye: [f32; 3] = state.character.camera_eye();
            let target: [f32; 3] = state.character.camera_target();
            let aspect = (logical_w / logical_h) as f32;
            let half_h = ene_vrm::camera::VIEWPORT_HEIGHT * 0.5;
            let half_w = half_h * aspect.max(0.0001);
            let ndc = glam::Vec2::new(ndc_x as f32, ndc_y as f32);
            let view = glam::camera::rh::view::look_at_mat4(
                eye.into(),
                target.into(),
                ene_vrm::camera::DEFAULT_UP.into(),
            );
            let view_pos = glam::Vec3::new(ndc.x * half_w, ndc.y * half_h, 0.0);
            let world_3d = view.inverse().transform_point3(view_pos);
            let ray_dir =
                glam::Vec3::new(target[0] - eye[0], target[1] - eye[1], target[2] - eye[2]);
            state.physics_world().cast_ray(world_3d, ray_dir, 100.0)
        } else {
            None
        };
        #[cfg(not(target_os = "windows"))]
        let hit: Option<crate::physics::RayHit> = None;

        hit
    } else {
        None
    };

    let cursor_over = hit.is_some();
    #[cfg_attr(
        not(target_os = "windows"),
        expect(unused_variables, reason = "documented exception for this lint")
    )]
    let allows_input = !transparent || cursor_over || drag_is_dragging;

    if let Some(lc) = local_cursor {
        *last_cursor = Some(lc);
        #[cfg(target_os = "linux")]
        {
            use crate::resource::cursor_state::CursorState;
            state.app.world_mut().resource_mut::<CursorState>().physical = Some(lc);
        }
    }

    #[cfg(target_os = "windows")]
    {
        let _ = cw.window.set_cursor_hittest(allows_input);
    }

    hit
}

const fn char_settings_hotkey_from_event(
    event: &WindowEvent,
    has_focus: bool,
) -> Option<SettingsAction> {
    if !has_focus {
        return None;
    }
    let WindowEvent::KeyboardInput {
        event:
            KeyEvent {
                physical_key: winit::keyboard::PhysicalKey::Code(code),
                state: ElementState::Pressed,
                ..
            },
        ..
    } = event
    else {
        return None;
    };
    use winit::keyboard::KeyCode;
    match code {
        KeyCode::KeyA => Some(SettingsAction::PrevCharacter),
        KeyCode::KeyD => Some(SettingsAction::NextCharacter),
        KeyCode::KeyW => Some(SettingsAction::PrevMotion),
        KeyCode::KeyS => Some(SettingsAction::NextMotion),
        _ => None,
    }
}

const fn key_pressed(event: &WindowEvent) -> Option<NamedKey> {
    if let WindowEvent::KeyboardInput {
        event:
            KeyEvent {
                logical_key: Key::Named(named),
                state: ElementState::Pressed,
                ..
            },
        ..
    } = event
    {
        Some(*named)
    } else {
        None
    }
}

const fn key_code_pressed(event: &WindowEvent) -> Option<winit::keyboard::KeyCode> {
    if let WindowEvent::KeyboardInput {
        event:
            KeyEvent {
                physical_key: winit::keyboard::PhysicalKey::Code(code),
                state: ElementState::Pressed,
                ..
            },
        ..
    } = event
    {
        Some(*code)
    } else {
        None
    }
}

const fn native_theme_override(preference: DesktopThemePreference) -> Option<winit::window::Theme> {
    match preference {
        DesktopThemePreference::System => None,
        DesktopThemePreference::Light => Some(winit::window::Theme::Light),
        DesktopThemePreference::Dark => Some(winit::window::Theme::Dark),
    }
}

fn apply_window_theme(window: &Window, preference: DesktopThemePreference) {
    // Seed the resolved palette first so newly-created windows never flash
    // the wrong decoration; System then removes the override to preserve OS
    // theme notifications.
    crate::theme::apply_native_theme(window);
    window.set_theme(native_theme_override(preference));
}

fn window_attributes(transparent: bool, fullscreen: Option<Fullscreen>) -> WindowAttributes {
    let mut attrs = WindowAttributes::default()
        .with_title("Ene")
        .with_resizable(true)
        .with_decorations(!transparent)
        .with_transparent(transparent)
        .with_window_level(WindowLevel::AlwaysOnTop);

    if let Some(fs) = fullscreen {
        attrs = attrs.with_fullscreen(Some(fs));
    } else {
        attrs = attrs.with_inner_size(LogicalSize::new(640.0, 480.0));
    }

    #[cfg(target_os = "windows")]
    {
        use winit::platform::windows::WindowAttributesExtWindows;
        attrs.with_no_redirection_bitmap(true)
    }
    #[cfg(not(target_os = "windows"))]
    {
        attrs
    }
}

struct CharacterWindow {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
}

impl CharacterWindow {
    fn new(
        window: Arc<Window>,
        instance: &wgpu::Instance,
        adapter: &wgpu::Adapter,
        device: &wgpu::Device,
        size: PhysicalSize<u32>,
    ) -> Result<Self, WindowSurfaceError> {
        let surface = instance
            .create_surface(window.clone())
            .map_err(|e| WindowSurfaceError::CreateSurface(e.to_string()))?;

        let caps = surface.get_capabilities(adapter);
        let (format, alpha_mode) = pick_format_and_alpha(&caps);

        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: wgpu::PresentMode::AutoVsync,
            alpha_mode,
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(device, &config);

        Ok(Self {
            window,
            surface,
            config,
        })
    }

    fn reconfigure(&mut self, device: &wgpu::Device, new_size: PhysicalSize<u32>) {
        if new_size.width == 0 || new_size.height == 0 {
            return;
        }
        self.config.width = new_size.width;
        self.config.height = new_size.height;
        self.surface.configure(device, &self.config);
    }

    /// Acquire the current surface texture, hand the resulting
    /// `TextureView` to `draw_fn`, and present the frame.
    fn with_surface_view(
        &self,
        draw_fn: impl FnOnce(&wgpu::TextureView),
    ) -> Result<(), AcquireError> {
        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame)
            | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => frame,
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => {
                return Err(AcquireError::Timeout);
            }
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                return Err(AcquireError::Reconfigure);
            }
            wgpu::CurrentSurfaceTexture::Validation => return Err(AcquireError::Fatal),
        };
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        draw_fn(&view);
        frame.present();
        Ok(())
    }
}

struct UiWindow {
    shell: crate::egui_shell::EguiWindowShell,
    settings_ui: SettingsUi,
}

impl UiWindow {
    fn new(
        window: Arc<Window>,
        instance: &wgpu::Instance,
        adapter: &wgpu::Adapter,
        device: &wgpu::Device,
        size: PhysicalSize<u32>,
    ) -> Result<Self, WindowSurfaceError> {
        let shell =
            crate::egui_shell::EguiWindowShell::new(window, instance, adapter, device, size)?;
        Ok(Self {
            shell,
            settings_ui: SettingsUi::new(),
        })
    }

    fn render_frame(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        settings: &mut CharacterSettings,
        ai: Option<&Arc<AiBridge>>,
        world: &mut bevy_ecs::world::World,
        ui_entity: bevy_ecs::entity::Entity,
        now_secs: f64,
    ) -> Result<(), AcquireError> {
        let Self { shell, settings_ui } = self;
        shell.render_frame(device, queue, egui::Id::new("settings_panel"), |ui| {
            settings_ui.render(ui, settings, ai, world, ui_entity, now_secs);
        })
    }
}

impl std::ops::Deref for UiWindow {
    type Target = crate::egui_shell::EguiWindowShell;

    fn deref(&self) -> &Self::Target {
        &self.shell
    }
}

impl std::ops::DerefMut for UiWindow {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.shell
    }
}

#[cfg(test)]
mod theme_tests {
    use super::*;

    #[test]
    fn native_theme_override_preserves_system_notifications() {
        assert_eq!(native_theme_override(DesktopThemePreference::System), None);
        assert_eq!(
            native_theme_override(DesktopThemePreference::Light),
            Some(winit::window::Theme::Light)
        );
        assert_eq!(
            native_theme_override(DesktopThemePreference::Dark),
            Some(winit::window::Theme::Dark)
        );
    }
}
