//! Winit event loop driver.
//!
//! Owns the winit window(s), their wgpu surfaces, and the
//! [`AppState`]. Implements winit 0.30's `ApplicationHandler` so
//! `EventLoop::run_app` is a single call from `main`.
//!
//! Each frame:
//!
//! 1. `about_to_wait` drains the cross-subsystem [`AppEvent`] bus,
//!    forwarding tray actions to UI state, applying AI inbox
//!    updates to the latest-response buffer, auto-popping the
//!    settings window on permission / user-input requests, ticking
//!    the GTK pump on Linux, and calling `flush_if_dirty` on the
//!    settings.
//! 2. `window_event` dispatches input. Keyboard `F1` toggles the
//!    settings window, `Escape` closes (and saves) the focused
//!    window, the window close button exits.
//! 3. Redraw happens via `request_redraw` from `about_to_wait` and
//!    is performed in `about_to_wait` to avoid winit 0.30's
//!    `RedrawRequested` double-fire on Windows.
use std::sync::Arc;
use std::time::{Duration, Instant};

use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, KeyEvent, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Fullscreen, Window, WindowAttributes, WindowId, WindowLevel};

use crate::ai_bridge::AiBridge;
use crate::events::{AppEvent, AppEventSender, TrayAction};
use crate::gpu::pick_format_and_alpha;
use crate::settings::{CharacterSettings, PendingPermission, PendingUserInput, QuestionDraft};
use crate::settings_ui::{PageKind, SettingsUi, widgets::SettingsAction};
use crate::state::AppState;
use device_query::DeviceQuery;

/// Top-level runtime. One per process.
pub struct Runtime {
    state: AppState,
    event_tx: AppEventSender,
    transparent: bool,
    char_window: Option<CharacterWindow>,
    ui_window: Option<UiWindow>,
    last_cursor_physical: Option<PhysicalPosition<f64>>,
    last_frame_instant: Option<Instant>,
    device_state: device_query::DeviceState,
    char_surface_fatal: bool,
    last_debug_update: Option<Instant>,
}

impl Runtime {
    pub fn new(state: AppState, event_tx: AppEventSender) -> Self {
        Self {
            state,
            event_tx,
            transparent: true,
            char_window: None,
            ui_window: None,
            last_cursor_physical: None,
            last_frame_instant: None,
            device_state: device_query::DeviceState::new(),
            char_surface_fatal: false,
            last_debug_update: None,
        }
    }

    fn show_settings_window(&mut self) {
        if let Some(uw) = self.ui_window.as_mut() {
            let mut ui_state = self.state.ui_state_mut();
            if !ui_state.settings_window_visible {
                ui_state.settings_window_visible = true;
                uw.settings_ui
                    .sync_from_settings(&self.state.settings, &ui_state);
                uw.window.set_visible(true);
                uw.window.request_redraw();
            }
        }
    }

    fn hide_settings_window(&mut self) {
        let mut ui_state = self.state.ui_state_mut();
        if ui_state.settings_window_visible {
            self.state.save();
            ui_state.settings_window_visible = false;
            if let Some(uw) = self.ui_window.as_mut() {
                uw.window.set_visible(false);
            }
        }
    }
}

impl ApplicationHandler for Runtime {
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
                if self.state.platform.wayland_region.is_none()
                    && let Some(ctx) =
                        crate::platform::wayland_region::WaylandInputRegionContext::try_new(
                            cw.window.as_ref(),
                        )
                {
                    self.state.platform.wayland_region = Some(ctx);
                }

                #[cfg(target_os = "linux")]
                if self.state.platform.layer_shell.is_none() {
                    self.state.platform.layer_shell =
                        Some(crate::platform::wayland_layer_shell::new_layer_shell_state());
                    let status = crate::platform::detect_layer_shell(&self.state);
                    tracing::info!(
                        target: "ene.linux.layer_shell",
                        available = matches!(
                            status,
                            crate::platform::wayland_layer_shell::LayerShellStatus::Available(_)
                        ),
                        "zwlr_layer_shell_v1 detection"
                    );
                }

                #[cfg(target_os = "linux")]
                if self.state.platform.mask_capture.is_none() {
                    let downsample = self.state.settings.graphics.mask_render_downsample;
                    if let Some(cam) = crate::platform::wayland_mask_capture::new_mask_capture_state(
                        &self.state.gpu.device,
                        char_size.width,
                        char_size.height,
                        downsample,
                    ) {
                        let device = Arc::new(self.state.gpu.device.clone());
                        let queue = Arc::new(self.state.gpu.queue.clone());
                        let worker = crate::platform::mask_readback::MaskReadbackWorker::spawn(
                            Arc::clone(&cam),
                            device,
                            queue,
                        );
                        self.state.platform.mask_readback_worker = Some(worker);
                        self.state.platform.mask_capture = Some(cam);
                    }
                }

                #[cfg(target_os = "linux")]
                if self.state.platform.x11_ctx.is_none()
                    && let Some(ctx) =
                        crate::platform::x11_taskbar::X11Context::try_new(cw.window.as_ref())
                {
                    self.state.platform.x11_ctx = Some(ctx);
                    tracing::info!(
                        target: "ene.linux.x11",
                        connected = true,
                        "X11 probe"
                    );
                }

                #[cfg(target_os = "windows")]
                {
                    let actual_scale = self.state.character.auto_fit_scale(0.9)
                        * self.state.settings.character_state.model_scale;
                    let specs = self
                        .state
                        .character
                        .build_character_bone_specs(actual_scale);
                    if !specs.is_empty() {
                        self.state
                            .physics
                            .add_character_bone_colliders(self.state.character_entity, &specs);
                    }
                }

                let motion_rel = self.state.settings.current_motion();
                let motion_path = self.state.settings.assets_dir.join(motion_rel);
                self.state.character.play_motion(&motion_path);
                cw.window.request_redraw();
                self.char_window = Some(cw);
            }
            Err(e) => {
                tracing::error!("Failed to create CharacterWindow: {e}");
                event_loop.exit();
                return;
            }
        }

        let ui_attrs = WindowAttributes::default()
            .with_title("ene UI")
            .with_inner_size(LogicalSize::new(460.0, 620.0))
            .with_resizable(true);
        let ui_w = match event_loop.create_window(ui_attrs) {
            Ok(w) => Arc::new(w),
            Err(e) => {
                tracing::error!("Failed to create ui window: {e}");
                event_loop.exit();
                return;
            }
        };
        let ui_size = ui_w.inner_size();
        match UiWindow::new(
            ui_w,
            &self.state.gpu.instance,
            &self.state.gpu.adapter,
            &self.state.gpu.device,
            ui_size,
        ) {
            Ok(uw) => {
                let visible = self.state.ui_state().settings_window_visible;
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

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        window_id: WindowId,
        event: WindowEvent,
    ) {
        let is_char = self
            .char_window
            .as_ref()
            .map(|w| w.window.id() == window_id)
            .unwrap_or(false);
        let is_ui = self
            .ui_window
            .as_ref()
            .map(|w| w.window.id() == window_id)
            .unwrap_or(false);

        if is_ui {
            self.handle_ui_window_event(event_loop, event);
        } else if is_char {
            self.handle_char_window_event(event_loop, event);
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let mut pending_permission: Option<PendingPermission> = None;
        let mut pending_user_input: Option<PendingUserInput> = None;
        while let Ok(event) = self.state.event_rx.try_recv() {
            match event {
                AppEvent::Tray(TrayAction::OpenSettings { page }) => {
                    self.show_settings_window();
                    if let Some(page) = page
                        && let Some(uw) = self.ui_window.as_mut()
                    {
                        uw.settings_ui.show(page);
                    }
                }
                AppEvent::Tray(TrayAction::Quit) => {
                    self.state.save();
                    event_loop.exit();
                    return;
                }
                AppEvent::Quit => {
                    self.state.save();
                    event_loop.exit();
                    return;
                }
                AppEvent::Ai(update) => match update {
                    crate::events::AiStreamUpdate::TextDelta(text) => {
                        self.state.ui_state_mut().ai_latest_response.push_str(&text);
                    }
                    crate::events::AiStreamUpdate::Finished
                    | crate::events::AiStreamUpdate::Error(_) => {}
                    crate::events::AiStreamUpdate::PermissionRequired {
                        request_id,
                        action,
                        target,
                        description,
                    } => {
                        pending_permission = Some(PendingPermission {
                            request_id,
                            action,
                            target,
                            description: if description.is_empty() {
                                None
                            } else {
                                Some(description)
                            },
                        });
                        self.state.ui_state_mut().settings_window_visible = true;
                        if let Some(uw) = self.ui_window.as_mut() {
                            uw.settings_ui.show(crate::settings_ui::PageKind::Ai);
                        }
                    }
                    crate::events::AiStreamUpdate::UserInputRequired { request_id, prompt } => {
                        pending_user_input = Some(PendingUserInput { request_id, prompt });
                        self.state.ui_state_mut().settings_window_visible = true;
                        if let Some(uw) = self.ui_window.as_mut() {
                            uw.settings_ui.show(crate::settings_ui::PageKind::Ai);
                        }
                    }
                    _ => {}
                },
                AppEvent::EmoteToken(name) => {
                    let now_secs = self
                        .ui_window
                        .as_ref()
                        .map(|uw| uw.settings_ui.started_at.elapsed().as_secs_f64())
                        .unwrap_or(0.0);
                    if let Some(uw) = self.ui_window.as_mut() {
                        uw.settings_ui
                            .emotion_queue
                            .push(crate::character_state::EmotionCommand {
                                emotion: name,
                                target_time: now_secs,
                                hold_secs: 4.0,
                                weight: 1.0,
                            });
                    }
                }
            }
        }
        if let Some(perm) = pending_permission {
            self.state.ui_state_mut().pending_permission = Some(perm);
        }
        if let Some(pui) = pending_user_input {
            let mut ui_state = self.state.ui_state_mut();
            ui_state.user_input_drafts = (0..pui.prompt.items.len())
                .map(|_| QuestionDraft::default())
                .collect();
            ui_state.pending_user_input = Some(pui);
        }

        if let Some(uw) = self.ui_window.as_mut() {
            let now_secs = uw.settings_ui.started_at.elapsed().as_secs_f64();
            let AppState {
                ref mut character, ..
            } = self.state;
            character.apply_emotions(&mut uw.settings_ui.emotion_queue, now_secs);
        }

        self.state.settings.flush_if_dirty();

        if let Some(_tray) = self.state.tray.as_ref() {
            #[cfg(target_os = "linux")]
            _tray.tick_gtk();
        }

        let cs = &self.state.settings.character_state;
        let auto_scale = self.state.character.auto_fit_scale(0.9);
        let actual_scale = auto_scale * cs.model_scale;
        if let Ok(transform) = self
            .state
            .world
            .query_one_mut::<&mut crate::physics::Transform>(self.state.character_entity)
        {
            transform.translation = cs.character_position;
            transform.scale = actual_scale;
        }

        #[cfg(target_os = "windows")]
        {
            let bone_poses = self.state.character.current_bone_poses();
            if !bone_poses.is_empty() {
                self.state.physics.update_character_bone_positions(
                    self.state.character_entity,
                    &bone_poses,
                    cs.character_position,
                    actual_scale,
                );
            }
            self.state.physics.step();
        }

        if let Some(cw) = self.char_window.as_ref() {
            let drag_is_dragging = self.state.character.drag.is_dragging();
            let debug_fps = self.state.settings.graphics.debug_fps;

            let should_update = if drag_is_dragging {
                self.last_debug_update = None;
                true
            } else if debug_fps == 0 {
                true
            } else {
                let now = Instant::now();
                match self.last_debug_update {
                    Some(last) => {
                        let interval = std::time::Duration::from_secs_f64(1.0 / debug_fps as f64);
                        if now.duration_since(last) >= interval {
                            self.last_debug_update = Some(now);
                            true
                        } else {
                            false
                        }
                    }
                    None => {
                        self.last_debug_update = Some(now);
                        true
                    }
                }
            };

            if should_update {
                self.state.debug.last_raycast_hit = update_char_window_cursor_and_hittest(
                    &mut self.state,
                    &self.device_state,
                    cw,
                    self.transparent,
                    drag_is_dragging,
                    &mut self.last_cursor_physical,
                );

                #[cfg(target_os = "windows")]
                let hovered_name = if let Some(hit) = self.state.debug.last_raycast_hit
                    && hit.entity == self.state.character_entity
                {
                    let colliders = self
                        .state
                        .physics
                        .colliders_for(self.state.character_entity);
                    if let Some(idx) = colliders.iter().position(|&h| h == hit.collider) {
                        self.state.character.get_active_bone_name(idx)
                    } else {
                        None
                    }
                } else {
                    None
                };
                #[cfg(not(target_os = "windows"))]
                let hovered_name = None;

                self.state.ui_state_mut().hovered_bone_name = hovered_name;
            }
        }

        if let Err(AcquireError::Fatal) = self.render_char_frame() {}
        if let Some(uw) = self.ui_window.as_mut() {
            let visible = self.state.ui_state().settings_window_visible;
            if uw.window.is_visible() != Some(visible) {
                uw.window.set_visible(visible);
            }
            if visible {
                uw.window.request_redraw();
                match uw.render_frame(
                    &self.state.gpu.device,
                    &self.state.gpu.queue,
                    &mut self.state.settings,
                    &self.state.ai,
                    &mut self.state.world,
                    self.state.ui_entity,
                ) {
                    Ok(_) => {}
                    Err(e) => match e {
                        AcquireError::Reconfigure => {
                            uw.reconfigure(&self.state.gpu.device, uw.window.inner_size());
                        }
                        AcquireError::Timeout => {}
                        AcquireError::Fatal => {
                            event_loop.exit();
                        }
                    },
                }
            }
        }

        if self.char_surface_fatal {
            event_loop.exit();
            return;
        }
        let target_fps = self.state.settings.graphics.target_fps;
        if target_fps == 0 {
            event_loop.set_control_flow(ControlFlow::Poll);
        } else {
            let frame_interval = Duration::from_secs_f64(f64::from(target_fps).recip());
            let last = self.last_frame_instant.unwrap_or_else(Instant::now);
            let next_deadline = last + frame_interval;
            event_loop.set_control_flow(ControlFlow::WaitUntil(next_deadline));
        }
    }
}

impl Runtime {
    fn handle_char_window_event(&mut self, event_loop: &ActiveEventLoop, event: WindowEvent) {
        let Some(cw) = self.char_window.as_mut() else {
            return;
        };
        match event {
            WindowEvent::CloseRequested => {
                self.state.save();
                event_loop.exit();
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
                if let Some(mask) = self.state.platform.mask_capture.as_ref() {
                    let downsample = self.state.settings.graphics.mask_render_downsample;
                    let mut guard = mask.lock();
                    let _ = guard.resize(&gpu.device, new_size.width, new_size.height, downsample);
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
                let AppState {
                    ref mut character, ..
                } = self.state;
                let event = match btn_state {
                    ElementState::Pressed => DragButtonEvent::Pressed,
                    ElementState::Released => DragButtonEvent::Released,
                };
                let eye = character.camera_eye();
                let target = character.camera_target();
                let cursor_world_2d = cursor_world_2d_for_char_window(cw, eye, target, cursor_phys);
                #[cfg(target_os = "windows")]
                let cursor_over = {
                    let scale = cw.window.scale_factor();
                    let logical_size = cw.window.inner_size().to_logical::<f64>(scale);
                    let logical = cursor_phys.to_logical::<f64>(scale);
                    let ndc_x = (logical.x / logical_size.width.max(1.0)) * 2.0 - 1.0;
                    let ndc_y = -((logical.y / logical_size.height.max(1.0)) * 2.0 - 1.0);
                    let aspect = (logical_size.width / logical_size.height.max(0.0001)) as f32;
                    let half_h = ene_vrm::camera::VIEWPORT_HEIGHT * 0.5;
                    let half_w = half_h * aspect;
                    let view = glam::Mat4::look_at_rh(
                        eye.into(),
                        target.into(),
                        ene_vrm::camera::DEFAULT_UP.into(),
                    );
                    let view_pos =
                        glam::Vec3::new(ndc_x as f32 * half_w, ndc_y as f32 * half_h, 0.0);
                    let world_3d = view.inverse().transform_point3(view_pos);
                    let ray_origin =
                        rapier3d::prelude::Point::new(world_3d.x, world_3d.y, world_3d.z);
                    let ray_dir = rapier3d::prelude::Vector::new(
                        target[0] - eye[0],
                        target[1] - eye[1],
                        target[2] - eye[2],
                    );
                    self.state
                        .physics
                        .cast_ray(ray_origin, ray_dir, 100.0)
                        .is_some_and(|hit| hit.entity == self.state.character_entity)
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
                        self.state.save();
                        event_loop.exit();
                    } else if matches!(named, NamedKey::F1) {
                        let visible = self.state.ui_state().settings_window_visible;
                        if visible {
                            self.hide_settings_window();
                        } else {
                            self.show_settings_window();
                        }
                    } else if key_code_pressed(&event) == Some(winit::keyboard::KeyCode::F3) {
                        let mut ui_state = self.state.ui_state_mut();
                        ui_state.show_collider_debug = !ui_state.show_collider_debug;
                        cw.window.request_redraw();
                    } else if key_code_pressed(&event) == Some(winit::keyboard::KeyCode::F8) {
                        #[cfg(target_os = "linux")]
                        {
                            self.state.platform.layer_shell_freeze =
                                !self.state.platform.layer_shell_freeze;
                            tracing::info!(
                                target: "ene.linux.layer_shell",
                                freeze = self.state.platform.layer_shell_freeze,
                                "char window freeze toggled"
                            );
                        }
                        cw.window.request_redraw();
                    } else if key_code_pressed(&event) == Some(winit::keyboard::KeyCode::F9) {
                        {
                            let next_val = {
                                let mut ui_state = self.state.ui_state_mut();
                                ui_state.show_input_region_debug =
                                    !ui_state.show_input_region_debug;
                                ui_state.show_input_region_debug
                            };
                            tracing::info!(
                                target: "ene.linux.input_region",
                                show = next_val,
                                "input-region debug overlay toggled"
                            );
                        }
                        cw.window.request_redraw();
                    } else {
                        let visible = self.state.ui_state().settings_window_visible;
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
            WindowEvent::RedrawRequested => {}
            _ => {}
        }
    }

    /// Render the character window directly from about_to_wait.
    ///
    /// Returns `Err(AcquireError::Fatal)` only on a fatal
    /// surface error; `Reconfigure` and `Timeout` are handled
    /// inline and return `Ok(())`.
    fn render_char_frame(&mut self) -> Result<(), AcquireError> {
        let Some(cw) = self.char_window.as_mut() else {
            return Ok(());
        };
        let transparent = self.transparent;
        let (show_collider_debug, show_mask_gizmo, show_input_region_debug) = {
            let ui_state = self.state.ui_state();
            (
                ui_state.show_collider_debug,
                ui_state.debug_overlay_visible,
                ui_state.show_input_region_debug,
            )
        };
        let last_raycast_hit = self.state.debug.last_raycast_hit;
        let character_entity = self.state.character_entity;
        let AppState {
            ref mut character,
            ref mut physics,
            ref mut debug,
            ref gpu,
            ref settings,
            ..
        } = self.state;
        let debug_renderer = &mut debug.debug_renderer;
        let (device, queue) = (&gpu.device, &gpu.queue);

        let cs = &settings.character_state;
        let actual_scale = character.auto_fit_scale(0.9) * cs.model_scale;
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

        if let Some(palette) = character.update_motion(dt_secs) {
            character.update_skin_palette_gpu(queue, &palette);
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

        let result = cw.with_surface_view(|view| {
            let swapchain_size = (cw.config.width, cw.config.height);
            let aa_mode = settings.graphics.antialiasing_mode;
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
                #[cfg_attr(target_os = "windows", expect(unused_variables))]
                let view_z = -camera_distance;
                let cam_view = glam::Mat4::look_at_rh(
                    camera_eye,
                    camera_target,
                    ene_vrm::camera::DEFAULT_UP.into(),
                );
                #[cfg_attr(target_os = "windows", expect(unused_variables))]
                let view_inverse = cam_view.inverse();
                let mut lines = Vec::new();
                if show_collider_debug {
                    let (hit_collider, hit_point) = match last_raycast_hit {
                        Some(h) if h.entity == character_entity => {
                            (Some(h.collider), Some(h.point))
                        }
                        _ => (None, None),
                    };
                    crate::raycast_debug::build_collider_lines(
                        &mut lines,
                        physics,
                        character_entity,
                        hit_collider,
                        hit_point,
                        true,
                    );
                    if let Some(model) = character.model() {
                        let center = glam::Vec3::from(model.center());
                        let normalize_scale = model.normalize_scale();
                        let auto_scale = character.auto_fit_scale(0.9);
                        let actual_scale = auto_scale * cs.model_scale;

                        for (bone_name, entry) in model.humanoid.iter() {
                            let parent_pos_raw = model.nodes.world_positions[entry.node];
                            let parent_world = cs.character_position
                                + (parent_pos_raw - center) * normalize_scale * actual_scale;

                            if let Some(child_node) =
                                crate::character::collider::get_humanoid_child_node(
                                    bone_name.as_str(),
                                    &model.humanoid,
                                )
                            {
                                let child_pos_raw = model.nodes.world_positions[child_node];
                                let child_world = cs.character_position
                                    + (child_pos_raw - center) * normalize_scale * actual_scale;

                                lines.push(ene_vrm::DebugLine {
                                    a: parent_world,
                                    b: child_world,
                                    color: glam::Vec4::new(1.0, 0.0, 0.0, 1.0),
                                });
                            } else {
                                let ext = 0.01 * actual_scale;
                                lines.push(ene_vrm::DebugLine {
                                    a: parent_world - glam::Vec3::X * ext,
                                    b: parent_world + glam::Vec3::X * ext,
                                    color: glam::Vec4::new(1.0, 0.0, 0.0, 1.0),
                                });
                                lines.push(ene_vrm::DebugLine {
                                    a: parent_world - glam::Vec3::Y * ext,
                                    b: parent_world + glam::Vec3::Y * ext,
                                    color: glam::Vec4::new(1.0, 0.0, 0.0, 1.0),
                                });
                                lines.push(ene_vrm::DebugLine {
                                    a: parent_world - glam::Vec3::Z * ext,
                                    b: parent_world + glam::Vec3::Z * ext,
                                    color: glam::Vec4::new(1.0, 0.0, 0.0, 1.0),
                                });
                            }
                        }

                        if let Some(spring_bones) = &model.spring_bones {
                            for chain in &spring_bones.springs {
                                for i in 0..chain.joints.len() - 1 {
                                    let parent_node = chain.joints[i].node;
                                    let child_node = chain.joints[i + 1].node;
                                    let parent_pos_raw = model.nodes.world_positions[parent_node];
                                    let child_pos_raw = model.nodes.world_positions[child_node];
                                    let parent_world = cs.character_position
                                        + (parent_pos_raw - center)
                                            * normalize_scale
                                            * actual_scale;
                                    let child_world = cs.character_position
                                        + (child_pos_raw - center) * normalize_scale * actual_scale;
                                    lines.push(ene_vrm::DebugLine {
                                        a: parent_world,
                                        b: child_world,
                                        color: glam::Vec4::new(1.0, 0.0, 0.0, 1.0),
                                    });
                                }
                            }
                        }
                    }
                }
                if show_mask_gizmo {
                    #[cfg(target_os = "linux")]
                    if let Some(mask) = self.state.platform.mask_capture.as_ref() {
                        crate::mask_gizmo::build_mask_rect_lines(
                            &mut lines,
                            mask,
                            cw.config.width,
                            cw.config.height,
                            view_inverse,
                            view_z,
                        );
                    }
                }

                if show_input_region_debug {
                    #[cfg(target_os = "linux")]
                    crate::input_region_debug::build_input_region_debug_lines(
                        &mut lines,
                        &self.state.platform.last_applied_input_rects,
                        self.state.platform.last_input_source,
                        cw.config.width,
                        cw.config.height,
                        view_inverse,
                        view_z,
                    );
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
        if let Some(mask) = self.state.platform.mask_capture.as_ref() {
            let mut mask_guard = mask.lock();
            let downsample = settings.graphics.mask_render_downsample;
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
            drop(mask_guard);
            queue.submit(std::iter::once(encoder.finish()));
            #[cfg(target_os = "linux")]
            if let Some(worker) = self.state.platform.mask_readback_worker.as_ref() {
                worker.request_readback();
            }
        }
        match result {
            Ok(()) => Ok(()),
            Err(AcquireError::Reconfigure) => {
                tracing::warn!("Character Surface acquire Outdated/Lost; reconfiguring");
                cw.reconfigure(device, cw.window.inner_size());
                Ok(())
            }
            Err(AcquireError::Timeout) => Ok(()),
            Err(AcquireError::Fatal) => {
                tracing::error!("Character Surface acquire failed fatally; exiting");
                self.char_surface_fatal = true;
                Ok(())
            }
        }
    }

    fn handle_ui_window_event(&mut self, _event_loop: &ActiveEventLoop, event: WindowEvent) {
        let uw = self.ui_window.as_mut().unwrap();
        let response = uw.egui_state.on_window_event(&uw.window, &event);
        if response.repaint {
            uw.window.request_redraw();
        }
        if response.consumed {
            return;
        }
        match event {
            WindowEvent::CloseRequested => {
                self.state.save();
                self.state.ui_state_mut().settings_window_visible = false;
            }
            WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. } => {
                uw.reconfigure(&self.state.gpu.device, uw.window.inner_size());
                uw.window.request_redraw();
            }
            WindowEvent::KeyboardInput { .. } => {
                if let Some(NamedKey::Escape) = key_pressed(&event) {
                    self.state.save();
                    self.state.ui_state_mut().settings_window_visible = false;
                }
            }
            WindowEvent::RedrawRequested => {}
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
            ref mut world,
            ui_entity,
            ..
        } = self.state;
        let now_secs = uw.settings_ui.started_at.elapsed().as_secs_f64();
        crate::settings_ui::widgets::apply_action(
            action,
            settings,
            &mut uw.settings_ui.animation,
            ai,
            world,
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
) -> Option<crate::physics::RaycastHit> {
    let mouse = device_state.get_mouse();
    let (gx, gy) = (mouse.coords.0, mouse.coords.1);

    let mut local_cursor = None;
    let hit = if let Ok(outer) = cw.window.outer_position() {
        let local_physical_x = gx as f64 - outer.x as f64;
        let local_physical_y = gy as f64 - outer.y as f64;

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
            let view = glam::Mat4::look_at_rh(
                eye.into(),
                target.into(),
                ene_vrm::camera::DEFAULT_UP.into(),
            );
            let view_pos = glam::Vec3::new(ndc.x * half_w, ndc.y * half_h, 0.0);
            let world_3d = view.inverse().transform_point3(view_pos);
            let ray_origin = rapier3d::prelude::Point::new(world_3d.x, world_3d.y, world_3d.z);
            let ray_dir = rapier3d::prelude::Vector::new(
                target[0] - eye[0],
                target[1] - eye[1],
                target[2] - eye[2],
            );
            state.physics.cast_ray(ray_origin, ray_dir, 100.0)
        } else {
            None
        };
        #[cfg(not(target_os = "windows"))]
        let hit: Option<crate::physics::RaycastHit> = None;

        hit
    } else {
        None
    };

    let cursor_over = hit.is_some_and(|h| h.entity == state.character_entity);

    let allows_input = !transparent || cursor_over || drag_is_dragging;

    if let Some(lc) = local_cursor {
        *last_cursor = Some(lc);
    }

    #[cfg(target_os = "windows")]
    {
        let _ = cw.window.set_cursor_hittest(allows_input);
    }
    #[cfg(target_os = "linux")]
    {
        crate::platform::apply_linux_click_through(
            state,
            allows_input,
            cursor_over,
            state.platform.layer_shell_freeze,
        );
    }

    hit
}

fn char_settings_hotkey_from_event(event: &WindowEvent, has_focus: bool) -> Option<SettingsAction> {
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

fn key_pressed(event: &WindowEvent) -> Option<NamedKey> {
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

fn key_code_pressed(event: &WindowEvent) -> Option<winit::keyboard::KeyCode> {
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

fn window_attributes(transparent: bool, fullscreen: Option<Fullscreen>) -> WindowAttributes {
    let mut attrs = WindowAttributes::default()
        .with_title("ene v2 (tw-test wgpu port)")
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
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let surface = instance
            .create_surface(window.clone())
            .map_err(|e| format!("Failed to create wgpu surface: {e}"))?;

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
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    egui_ctx: egui::Context,
    egui_state: egui_winit::State,
    egui_renderer: egui_wgpu::Renderer,
    settings_ui: SettingsUi,
    textures_to_free: Vec<Vec<egui::TextureId>>,
}

impl UiWindow {
    fn new(
        window: Arc<Window>,
        instance: &wgpu::Instance,
        adapter: &wgpu::Adapter,
        device: &wgpu::Device,
        size: PhysicalSize<u32>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let surface = instance
            .create_surface(window.clone())
            .map_err(|e| format!("Failed to create wgpu surface: {e}"))?;

        let caps = surface.get_capabilities(adapter);
        let format = *caps
            .formats
            .iter()
            .find(|f| !f.is_srgb())
            .unwrap_or(&caps.formats[0]);
        let alpha_mode = caps.alpha_modes[0];

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

        let egui_ctx = egui::Context::default();
        let viewport_id = egui::ViewportId::ROOT;
        let egui_state = egui_winit::State::new(
            egui_ctx.clone(),
            viewport_id,
            &window,
            Some(window.scale_factor() as f32),
            None,
            Some(device.limits().max_texture_dimension_2d as usize),
        );
        let egui_renderer =
            egui_wgpu::Renderer::new(device, format, egui_wgpu::RendererOptions::default());

        Ok(Self {
            window,
            surface,
            config,
            egui_ctx,
            egui_state,
            egui_renderer,
            settings_ui: SettingsUi::new(),
            textures_to_free: vec![Vec::new(), Vec::new(), Vec::new()],
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

    fn render_frame(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        settings: &mut CharacterSettings,
        ai: &Arc<AiBridge>,
        world: &mut hecs::World,
        ui_entity: hecs::Entity,
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

        let raw_input = self.egui_state.take_egui_input(&self.window);
        self.egui_ctx.begin_pass(raw_input);

        let mut panel_ui = egui::Ui::new(
            self.egui_ctx.clone(),
            egui::Id::new("settings_panel"),
            egui::UiBuilder::new()
                .layer_id(egui::LayerId::background())
                .max_rect(self.egui_ctx.content_rect()),
        );
        panel_ui.set_clip_rect(self.egui_ctx.content_rect());

        egui::CentralPanel::default().show_inside(&mut panel_ui, |ui| {
            self.settings_ui.render(ui, settings, ai, world, ui_entity);
        });

        let full_output = self.egui_ctx.end_pass();
        let platform_output = full_output.platform_output;
        self.egui_state
            .handle_platform_output(&self.window, platform_output);

        let tris = self
            .egui_ctx
            .tessellate(full_output.shapes, full_output.pixels_per_point);

        for (id, image_delta) in &full_output.textures_delta.set {
            self.egui_renderer
                .update_texture(device, queue, *id, image_delta);
        }

        let mut encoder =
            device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });

        let screen_descriptor = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [self.config.width, self.config.height],
            pixels_per_point: self.window.scale_factor() as f32,
        };

        let user_cmds = self.egui_renderer.update_buffers(
            device,
            queue,
            &mut encoder,
            &tris,
            &screen_descriptor,
        );

        {
            let mut rp = encoder
                .begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: None,
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &view,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                            store: wgpu::StoreOp::Store,
                        },
                        depth_slice: None,
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                })
                .forget_lifetime();

            self.egui_renderer
                .render(&mut rp, &tris, &screen_descriptor);
        }

        queue.submit(
            user_cmds
                .into_iter()
                .chain(std::iter::once(encoder.finish())),
        );

        let to_free_now = self.textures_to_free.remove(0);
        for id in to_free_now {
            self.egui_renderer.free_texture(&id);
        }
        self.textures_to_free.push(full_output.textures_delta.free);

        frame.present();
        Ok(())
    }
}

enum AcquireError {
    Reconfigure,
    Timeout,
    Fatal,
}
