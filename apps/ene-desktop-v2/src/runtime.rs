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
use std::time::Instant;

use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, KeyEvent, MouseButton, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowAttributes, WindowId, WindowLevel};

use crate::ai_bridge::AiBridge;
use crate::events::{AppEvent, AppEventSender, TrayAction};
use crate::gpu::pick_format_and_alpha;
use crate::settings::{CharacterSettings, PendingPermission, PendingUserInput, QuestionDraft};
use crate::settings_ui::{PageKind, SettingsUi};
use crate::state::AppState;
use device_query::DeviceQuery;

/// Top-level runtime. One per process.
pub struct Runtime {
    state: AppState,
    /// Clone of the cross-subsystem sender; used to push `Quit`
    /// from inside event handlers that only have `&mut self`.
    event_tx: AppEventSender,
    /// `true` = transparent window, no decorations, clear to
    /// `(0,0,0,0)`. `false` = opaque gray window with title bar.
    transparent: bool,
    char_window: Option<CharacterWindow>,
    ui_window: Option<UiWindow>,
    /// PR4.2: last cursor position in physical pixels (only
    /// populated when the character window has cursor events).
    last_cursor_physical: Option<PhysicalPosition<f64>>,
    /// PR4.2: monotonic clock for `dt_secs` smoothing.
    last_frame_instant: Option<Instant>,
    /// PR4.2 follow-up: one-shot diagnostic log on the very
    /// first `RedrawRequested` so we can verify surface / depth /
    /// camera-aspect / model-uniform are in sync.
    diagnostics_logged: bool,
    /// PR5.1: global mouse position poll, refreshed every frame in
    /// `about_to_wait` regardless of whether the character window
    /// is currently receiving events (so the click-through hit test
    /// stays correct while the window is in the click-through state
    /// and the OS stops delivering `CursorMoved` to it).
    device_state: device_query::DeviceState,
}

impl Runtime {
    pub fn new(state: AppState, event_tx: AppEventSender) -> Self {
        // Default to transparent mode (matches the legacy
        // Bevy app's behavior: undecorated, per-pixel alpha so
        // the desktop shows through). Pressing `Space` toggles
        // decorations + window transparency together so the
        // user can grab the title bar for debugging.
        Self {
            state,
            event_tx,
            transparent: true,
            char_window: None,
            ui_window: None,
            last_cursor_physical: None,
            last_frame_instant: None,
            diagnostics_logged: false,
            device_state: device_query::DeviceState::new(),
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
        event_loop.set_control_flow(winit::event_loop::ControlFlow::Poll);

        // Initialise the tray on first resume. The Windows backend
        // spawns its own pump thread; on Linux we just need to
        // register the icon (GTK pump runs in `about_to_wait`).
        self.state.init_tray(&self.event_tx);

        if self.char_window.is_some() {
            return;
        }

        // Create the character window
        let char_attrs = window_attributes(self.transparent);
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
                // PR3: load the default VRM and build the
                // render pipeline.
                self.state
                    .character
                    .init(&self.state.gpu.device, &self.state.gpu.queue, format);
                self.state
                    .character
                    .resize(&self.state.gpu.device, (char_size.width, char_size.height));
                // PR5.1: read the loaded mesh back from the
                // GPU and feed it to Rapier as a trimesh. The
                // PR5.2: one sphere collider per humanoid bone
                // (fingers auto-filtered). `update_motion`
                // (called every `about_to_wait`) keeps the bone
                // world positions fresh, so the colliders
                // follow the animation without any per-frame
                // readback. The radius is baked in
                // pre-scaled by `actual_scale` so the
                // collider matches the rendered mesh; one-time
                // cost at model load.
                let actual_scale = self.state.character.auto_fit_scale(0.9)
                    * self.state.settings.character_state.model_scale;
                let bones = self.state.character.build_character_bone_data(actual_scale);
                if !bones.is_empty() {
                    self.state
                        .physics
                        .add_character_bone_colliders(self.state.character_entity, &bones);
                }
                // PR4.16: load the default VRMA motion. The
                // resolved path comes from
                // `CharacterState::current_motion()` (CLI
                // override > selected_motion > first
                // available). The VRMA load is best-effort:
                // a missing file logs a warning and leaves
                // the model in its rest pose (which is what
                // the user sees with `default_motion = ""`).
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

        // Create the UI window. The settings UI is only revealed
        // when the user toggles it via F1 or the tray; the window
        // itself is always alive in the background.
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
        // 1. Drain the cross-subsystem bus. Tray actions update UI
        //    state; AI events drive the latest-response buffer and
        //    auto-popup. `Quit` exits.
        let mut pending_permission: Option<PendingPermission> = None;
        let mut pending_user_input: Option<PendingUserInput> = None;
        while let Ok(event) = self.state.event_rx.try_recv() {
            match event {
                AppEvent::Tray(TrayAction::OpenSettings) => {
                    self.show_settings_window();
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
                    | crate::events::AiStreamUpdate::Error(_) => {
                        // Latest response is appended-to as text
                        // deltas arrive; Finished/Error is the
                        // end-of-stream sentinel.
                    }
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
                            description,
                        });
                        self.state.ui_state_mut().settings_window_visible = true;
                    }
                    crate::events::AiStreamUpdate::UserInputRequired { request_id, prompt } => {
                        pending_user_input = Some(PendingUserInput { request_id, prompt });
                        self.state.ui_state_mut().settings_window_visible = true;
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

        // 1.5. PR4.4: drain the `EmotionQueue` and apply the
        //      resulting weights to the loaded VRM. Uses the
        //      same `started_at` clock as the `EmoteToken`
        //      producer above so timestamps line up. The
        //      renderer no-ops if the model failed to load.
        if let Some(uw) = self.ui_window.as_mut() {
            let now_secs = uw.settings_ui.started_at.elapsed().as_secs_f64();
            let AppState {
                ref mut character, ..
            } = self.state;
            character.apply_emotions(&mut uw.settings_ui.emotion_queue, now_secs);
        }

        // 2. Auto-save any settings that were marked dirty by UI
        //    handlers in this frame.
        self.state.settings.flush_if_dirty();

        // 3. Pump GTK on Linux so the tray stays alive.
        if let Some(_tray) = self.state.tray.as_ref() {
            #[cfg(target_os = "linux")]
            _tray.tick_gtk();
        }

        // 3.5. Sync ECS transform and step Rapier physics
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

        // PR5.2: push the animated bone positions into the
        // colliders that `add_character_bone_colliders`
        // registered at model load, and slide the underlying
        // body to `character_position` so the whole rig follows
        // drag and animation. The collider local positions are
        // scaled by `actual_scale` here so the world-space
        // spheres land in the same frame the per-frame model
        // matrix produces. The update happens *after* the model
        // transform has been resolved above so the body sees the
        // freshest `character_position`; the bone positions are
        // current because `update_skin_palette` (called from
        // `update_motion` in the `RedrawRequested` handler of
        // the previous iteration) writes them every frame.
        let bone_positions = self.state.character.current_bone_local_positions();
        if !bone_positions.is_empty() {
            self.state.physics.update_character_bone_positions(
                self.state.character_entity,
                &bone_positions,
                cs.character_position,
                actual_scale,
            );
        }
        self.state.physics.step();

        // PR5.1: per-frame click-through. `device_query` gives us
        // the global cursor in screen pixels regardless of which
        // window currently owns focus; combined with the
        // character window's outer position this yields the
        // window-local cursor even when the window is currently
        // click-through (and so is not receiving `CursorMoved`
        // events from winit). The hit test is a BVH-backed Rapier
        // raycast against the character trimesh, not a single
        // AABB cuboid.
        if let Some(cw) = self.char_window.as_ref() {
            update_char_window_cursor_and_hittest(
                &self.state,
                &self.device_state,
                cw,
                self.transparent,
                self.state.character.drag.is_dragging(),
                &mut self.last_cursor_physical,
            );
        }

        // 4. Trigger a redraw for both windows.
        if let Some(cw) = self.char_window.as_mut() {
            cw.window.request_redraw();
        }
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
    }
}

impl Runtime {
    fn handle_char_window_event(&mut self, event_loop: &ActiveEventLoop, event: WindowEvent) {
        // Take an owned snapshot of the pieces we need so the
        // borrow checker does not have to reason about the
        // `cw`/`self.state.character` split while we're in the
        // match arms below.
        let Some(cw) = self.char_window.as_mut() else {
            return;
        };
        match event {
            WindowEvent::CloseRequested => {
                self.state.save();
                event_loop.exit();
            }
            WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. } => {
                // PR4.2 fix: reconfigure BOTH the surface and the
                // depth texture. winit fires a `Resized` (often
                // with a different size than the `with_inner_size`
                // hint we passed at window creation — e.g. the OS
                // can clamp to a minimum, or the DPI scale has
                // shifted). If we only reconfigure the surface and
                // leave the depth texture at its initial 640×480,
                // the next `RedrawRequested` triggers
                // "Attachments have differing sizes" because the
                // surface texture and the depth view no longer
                // match.
                let AppState {
                    ref mut character,
                    ref gpu,
                    ..
                } = self.state;
                let new_size = cw.window.inner_size();
                cw.reconfigure(&gpu.device, new_size);
                character.resize(&gpu.device, (new_size.width, new_size.height));
                cw.window.request_redraw();
            }
            WindowEvent::CursorMoved { position, .. } => {
                // PR4.2: store the last cursor position (in logical
                // pixels) for the look-at projection. The actual
                // smoothing happens in `update_look_at` during
                // `RedrawRequested`, so the dt is correct.
                self.last_cursor_physical = Some(position);
                cw.window.request_redraw();

                // PR4.3: integrate the drag delta if the user is
                // currently dragging. The hit-test and the
                // position projection are computed against the
                // loaded character's transformed AABB.
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
                }
            }
            WindowEvent::MouseInput {
                state: btn_state,
                button: MouseButton::Left,
                ..
            } => {
                // PR4.3: start / end a drag on the left button. The
                // hit-test determines whether the press landed on
                // the character silhouette.
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
                // Re-run the per-bone hit test for the *event*
                // cursor position. The `about_to_wait` poll
                // may be a frame stale by the time `MouseInput`
                // fires; running it again here keeps the press
                // predicate synchronised with the actual click
                // coordinates.
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
                let view_pos = glam::Vec3::new(ndc_x as f32 * half_w, ndc_y as f32 * half_h, 0.0);
                let world_3d = view.inverse().transform_point3(view_pos);
                let ray_origin = rapier3d::prelude::Point::new(world_3d.x, world_3d.y, world_3d.z);
                let ray_dir = rapier3d::prelude::Vector::new(
                    target[0] - eye[0],
                    target[1] - eye[1],
                    target[2] - eye[2],
                );
                let cursor_over = self
                    .state
                    .physics
                    .cast_ray(ray_origin, ray_dir, 100.0)
                    .is_some_and(|(entity, _)| entity == self.state.character_entity);
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
                        // `set_decorations` toggles the title bar;
                        // `set_transparent` toggles per-pixel alpha
                        // (both are best-effort — on Windows the
                        // latter is fixed at window creation time
                        // because `WS_EX_LAYERED` is set then, but
                        // the call is still cheap and safe).
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
                    } else {
                        // Character-window WASD / Space shortcuts
                        // when the settings window is open on the
                        // Character page and egui is not focused.
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
            WindowEvent::RedrawRequested => {
                // PR3: the character window renders the VRM model
                // owned by `self.state.character`. The depth
                // texture and the wgpu pipeline are managed inside
                // `CharacterRenderer::render`. The window's surface
                // acquisition is hidden behind `with_surface_view`.
                //
                // The error path can't be a method call on
                // `&mut self` because `cw` is still borrowed
                // mutably here, so we inline it.
                let transparent = self.transparent;
                let AppState {
                    ref mut character,
                    ref gpu,
                    ref settings,
                    ..
                } = self.state;
                let (device, queue) = (&gpu.device, &gpu.queue);

                // PR4.2 follow-up: one-shot diagnostic so we can
                // see whether the char window size / surface config
                // / depth texture / camera aspect / model uniform
                // are all in agreement. Logs on the very first
                // RedrawRequested only.
                if !self.diagnostics_logged {
                    self.diagnostics_logged = true;
                    let phys = cw.window.inner_size();
                    let surface_w = cw.config.width;
                    let surface_h = cw.config.height;
                    let cs = &settings.character_state;
                    let auto_fit = character.auto_fit_scale(0.9);
                    let actual_scale_dbg = auto_fit * cs.model_scale;
                    let model_uniform_dbg = ene_vrm::ModelUniform::from_mat4(
                        character.model_matrix(cs.character_position, actual_scale_dbg),
                    );
                    let cam = character.camera_dbg();
                    let depth = character.depth_size_dbg();
                    let loaded = character.model_aabb_dbg();
                    // PR4.19 ultra-verbose: also dump the model's
                    // loader-derived values (center, normalize_scale,
                    // merged skeleton joint count) and the per-axis
                    // model-matrix scale, so we can see whether the
                    // runtime is computing the right matrix. The
                    // user is reporting the model is "5x taller than
                    // expected" with only the tip of the feet visible
                    // — this dump makes the cause obvious.
                    let model_scale_x = (model_uniform_dbg.model[0][0].powi(2)
                        + model_uniform_dbg.model[1][0].powi(2)
                        + model_uniform_dbg.model[2][0].powi(2))
                    .sqrt();
                    let model_scale_y = (model_uniform_dbg.model[0][1].powi(2)
                        + model_uniform_dbg.model[1][1].powi(2)
                        + model_uniform_dbg.model[2][1].powi(2))
                    .sqrt();
                    let model_scale_z = (model_uniform_dbg.model[0][2].powi(2)
                        + model_uniform_dbg.model[1][2].powi(2)
                        + model_uniform_dbg.model[2][2].powi(2))
                    .sqrt();
                    let model_translation = [
                        model_uniform_dbg.model[0][3],
                        model_uniform_dbg.model[1][3],
                        model_uniform_dbg.model[2][3],
                    ];
                    let merged_skel_joints = character.model_dbg_merged_skel_joints().unwrap_or(0);
                    let loader_center = character.model_dbg_center();
                    let loader_norm = character.model_dbg_normalize_scale();
                    // PR4.19: also dump the camera's view_proj
                    // matrix so we can verify the orthographic
                    // projection isn't adding a hidden scale (the
                    // user reports the model is "5x taller" and
                    // only the feet are visible; the model_matrix
                    // is correct, so the camera/projection might
                    // be the culprit).
                    let view_proj = character.camera_view_proj_dbg();
                    let vp_scale_x = (view_proj[0][0].powi(2)
                        + view_proj[1][0].powi(2)
                        + view_proj[2][0].powi(2))
                    .sqrt();
                    let vp_scale_y = (view_proj[0][1].powi(2)
                        + view_proj[1][1].powi(2)
                        + view_proj[2][1].powi(2))
                    .sqrt();
                    let vp_scale_z = (view_proj[0][2].powi(2)
                        + view_proj[1][2].powi(2)
                        + view_proj[2][2].powi(2))
                    .sqrt();
                    let vp_translation = [view_proj[0][3], view_proj[1][3], view_proj[2][3]];
                    let model_view_proj = character.model_view_proj_dbg(
                        [
                            cs.character_position.x,
                            cs.character_position.y,
                            cs.character_position.z,
                        ],
                        actual_scale_dbg,
                    );
                    let model_matrix_runtime = character.model_matrix_runtime_dbg(
                        [
                            cs.character_position.x,
                            cs.character_position.y,
                            cs.character_position.z,
                        ],
                        actual_scale_dbg,
                    );
                    let view_only = character.camera_view_dbg();
                    let proj_only = character.camera_proj_dbg();
                    tracing::info!(
                        "PR4.19-diag: model_view_proj={:?} model_matrix_runtime={:?} view_only={:?} proj_only={:?}",
                        model_view_proj,
                        model_matrix_runtime,
                        view_only,
                        proj_only,
                    );
                    tracing::info!(
                        "PR4.2-diag: char_win={}x{} scale_factor={:.3} surface_config={}x{} depth_texture={}x{} camera_aspect={:.3} char_pos={:?} user_model_scale={:.3} auto_fit_scale={:.3} actual_scale={:.3} model_uniform={:?} cam_eye={:?} cam_target={:?} cam_viewport_h={:.3} loaded_aabb={:?}",
                        phys.width,
                        phys.height,
                        cw.window.scale_factor(),
                        surface_w,
                        surface_h,
                        depth.0,
                        depth.1,
                        cam.0,
                        cs.character_position,
                        cs.model_scale,
                        auto_fit,
                        actual_scale_dbg,
                        model_uniform_dbg.model,
                        cam.1,
                        cam.2,
                        cam.3,
                        loaded,
                    );
                    tracing::info!(
                        "PR4.19-diag: loader_center={:?} loader_normalize_scale={:.4} merged_skel_joints={} model_scale=({:.4}, {:.4}, {:.4}) model_translation={:?} view_proj_scale=({:.4}, {:.4}, {:.4}) view_proj_translation={:?} model_view_proj={:?}",
                        loader_center,
                        loader_norm,
                        merged_skel_joints,
                        model_scale_x,
                        model_scale_y,
                        model_scale_z,
                        model_translation,
                        vp_scale_x,
                        vp_scale_y,
                        vp_scale_z,
                        vp_translation,
                        model_view_proj,
                    );
                }

                // PR4.1: compose the per-frame model transform
                // from `CharacterState`. `character_position` and
                // `model_scale` are clamped by `clamp_runtime_values`
                // on every mutation, so we can read them directly.
                //
                // PR-fix (model too big for viewport): the runtime
                // uses the per-frame `auto_fit_scale` (recomputed
                // from the loaded AABB and the current viewport
                // aspect) as the actual model scale. The user's
                // `settings.character_state.model_scale` is now a
                // hint of intent only — it is no longer passed
                // through to the GPU, so a `model_scale = 2.2` set
                // in a previous session can no longer blow the
                // model past the viewport. The user can still pan
                // via `character_position`; a future "zoom" slider
                // can be reintroduced as a separate knob that
                // multiplies on top of the fit (see
                // `CharacterRenderer::auto_fit_scale` docs).
                let cs = &settings.character_state;
                let actual_scale = character.auto_fit_scale(0.9) * cs.model_scale;
                // PR-fix (character pushed to top of viewport):
                // point the orthographic camera at the humanoid
                // body center (head → chest → hips → AABB center)
                // so the character is framed in the middle of the
                // window regardless of the AABB shape. Done before
                // the model matrix is composed so the camera and
                // the draw agree on `actual_scale`.
                //
                // PR-fix: `update_camera_target` now reads the
                // chest bone's *rest* translation, not the
                // previous frame's animated `world_positions`, so
                // a VRMA motion that oscillates the chest no
                // longer makes the camera (and therefore the
                // look-at output that depends on the camera)
                // jitter frame-to-frame.

                character.update_camera_target(actual_scale);
                let model_uniform = ene_vrm::ModelUniform::from_mat4(
                    character.model_matrix(cs.character_position, actual_scale),
                );

                // PR4.2: advance the cursor-driven head-look-at
                // state. The smoothed target is stored on the
                // renderer; PR4.5+ skinning will read it. Until
                // then the value is observable via the debug
                // `look_at_target` accessor and via
                // `body_tracking(strength)`.
                let now = Instant::now();
                let dt_secs = self
                    .last_frame_instant
                    .map_or(1.0 / 60.0, |t| now.duration_since(t).as_secs_f32())
                    .clamp(0.0, 0.1);
                self.last_frame_instant = Some(now);

                // PR4.16: advance the VRMA playback clock,
                // evaluate the active clip, push expression
                // weights into the morph layer, and write
                // the new skin palette to the GPU. The
                // returned `Vec<Mat4>` is forwarded to
                // `VrmRenderer::update_skin_palette` so the
                // bones move before the next render pass.
                // No-op when no motion is loaded, the
                // player is paused, or the model has no
                // joints.
                if let Some(palette) = character.update_motion(dt_secs) {
                    character.update_skin_palette_gpu(queue, &palette);
                }

                if let Some(cursor) = self.last_cursor_physical {
                    // PR4.8: the renderer reads the humanoid
                    // registry's `head` bone (when present) to
                    // derive the head world position. The
                    // runtime no longer pre-computes
                    // `head_world_for(...)` — that helper
                    // stayed as the fallback for models
                    // without humanoid metadata. We pass the
                    // model pivot + scale instead so the
                    // renderer can scale the bone's rest
                    // translation by `model_scale` and add
                    // the character position.
                    //
                    // PR-fix: the look-at must use the same
                    // `actual_scale` as the draw (auto-fit *
                    // user slider) so the head's world
                    // position lines up with what the user
                    // sees. Otherwise the head appears to
                    // track the cursor from a different point
                    // than the rendered model.
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
                    character.render(device, queue, view, transparent, &model_uniform);
                });
                if let Err(err) = result {
                    match err {
                        AcquireError::Reconfigure => {
                            tracing::warn!(
                                "Character Surface acquire Outdated/Lost; reconfiguring"
                            );
                            cw.reconfigure(&self.state.gpu.device, cw.window.inner_size());
                            cw.window.request_redraw();
                        }
                        AcquireError::Timeout => cw.window.request_redraw(),
                        AcquireError::Fatal => {
                            tracing::error!("Character Surface acquire failed fatally; exiting");
                            event_loop.exit();
                        }
                    }
                }
            }
            _ => {}
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
            WindowEvent::RedrawRequested => {
                // Rendering happens in `about_to_wait` to avoid
                // winit 0.30's `RedrawRequested` double-fire on
                // Windows.
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
        // Split-borrow: the runtime holds a `&mut AppState` so we
        // can hand separate `&mut` / `&` references to the
        // dispatcher.
        let AppState {
            ref mut settings,
            ref ai,
            ref mut world,
            ui_entity,
            ..
        } = self.state;
        crate::settings_ui::widgets::apply_action(
            action,
            settings,
            &mut uw.settings_ui.animation,
            ai,
            world,
            ui_entity,
        );
    }
}

fn cw_char_window_has_focus(cw: &CharacterWindow) -> bool {
    cw.window.has_focus()
}

/// PR4.3: compute the cursor's 2D world position for the drag
/// hit-test and the drag integration. `position` is the latest
/// winit `PhysicalPosition<f64>` (already plumbed via
/// `Runtime::last_cursor_physical`); the function converts it to
/// window-logical pixels (so the projection matches
/// `look_at::compute_world_target` which expects logical pixels) for the look-at projection.
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

/// PR5.1: per-frame click-through update for the character
/// window. Reads the global cursor via `device_query`, projects
/// it through the orthographic camera, casts a Rapier ray
/// against the character trimesh (BVH-accelerated), and toggles
/// winit's `set_cursor_hittest` so the rest of the desktop
/// receives the click when the cursor is not on the silhouette.
///
/// This is called every `about_to_wait`, including while the
/// window is in the click-through state, so the OS is not
/// delivering `CursorMoved` to it and the runtime must rely on
/// `device_query` for the global position.
fn update_char_window_cursor_and_hittest(
    state: &AppState,
    device_state: &device_query::DeviceState,
    cw: &CharacterWindow,
    transparent: bool,
    drag_is_dragging: bool,
    last_cursor: &mut Option<winit::dpi::PhysicalPosition<f64>>,
) {
    // 1. Global cursor in screen pixels.
    let mouse = device_state.get_mouse();
    let (gx, gy) = (mouse.coords.0, mouse.coords.1);

    // 2. Window's top-left in screen pixels (physical).
    //    `outer_position` returns `Result` because some Wayland
    //    compositors can't report it; we treat the failure as
    //    "cursor is outside the window" and let the NDC clip
    //    decide.
    let outer = match cw.window.outer_position() {
        Ok(p) => p,
        Err(_) => return,
    };
    let local_physical_x = gx as f64 - outer.x as f64;
    let local_physical_y = gy as f64 - outer.y as f64;

    // 3. Convert to logical pixels and NDC. The NDC is what the
    //    orthographic camera math consumes.
    let scale = cw.window.scale_factor();
    let logical_x = local_physical_x / scale;
    let logical_y = local_physical_y / scale;
    let inner = cw.window.inner_size();
    let logical_w = (inner.width as f64 / scale).max(1.0);
    let logical_h = (inner.height as f64 / scale).max(1.0);
    let ndc_x = (logical_x / logical_w) * 2.0 - 1.0;
    let ndc_y = -((logical_y / logical_h) * 2.0 - 1.0);

    // Outside the window's NDC rect? Skip the BVH raycast — the
    // window is click-through anyway.
    let inside_window = ndc_x.abs() <= 1.0 && ndc_y.abs() <= 1.0;

    let cursor_over = if inside_window {
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
        state
            .physics
            .cast_ray(ray_origin, ray_dir, 100.0)
            .is_some_and(|(entity, _)| entity == state.character_entity)
    } else {
        false
    };

    let allows_input = !transparent || cursor_over || drag_is_dragging;

    // Stash the latest window-local cursor for the press / drag
    // state machine (which is fed by winit's `CursorMoved` /
    // `MouseInput` events).
    *last_cursor = Some(winit::dpi::PhysicalPosition::new(
        local_physical_x,
        local_physical_y,
    ));

    // 4. winit handles the OS-level click-through for us.
    //    `allows_input == false` toggles `WS_EX_TRANSPARENT` on
    //    Windows and the platform equivalent elsewhere, so the
    //    click goes to the window underneath.
    let _ = cw.window.set_cursor_hittest(allows_input);
}

fn char_settings_hotkey_from_event(
    event: &WindowEvent,
    has_focus: bool,
) -> Option<crate::settings_ui::widgets::SettingsAction> {
    use crate::settings_ui::widgets::SettingsAction;
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

fn window_attributes(transparent: bool) -> WindowAttributes {
    let attrs = WindowAttributes::default()
        .with_title("ene v2 (tw-test wgpu port)")
        .with_inner_size(LogicalSize::new(640.0, 480.0))
        .with_resizable(true)
        .with_decorations(!transparent)
        .with_transparent(transparent)
        .with_window_level(WindowLevel::AlwaysOnTop);

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
    /// `TextureView` to `draw_fn`, and present the frame. Returns
    /// `Ok(())` on success and an [`AcquireError`] otherwise. The
    /// caller is responsible for handling the error path because
    /// the surface borrow conflicts with the borrow the error
    /// handler would need.
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

// `RectRenderer` and the red-quad smoke from PR0/1/2 are gone; the
// character window is now driven by `CharacterRenderer` (PR3).
