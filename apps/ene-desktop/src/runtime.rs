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
use winit::window::{Window, WindowAttributes, WindowId, WindowLevel};

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
    /// PR5.7: set to `true` when the char surface acquisition
    /// returns `AcquireError::Fatal`. The runtime exits the
    /// event loop at the tail of the next `about_to_wait` to
    /// avoid logging the same fatal every frame (the error
    /// path is now reached from `about_to_wait` instead of
    /// `RedrawRequested`, so we can't `event_loop.exit()`
    /// inline).
    char_surface_fatal: bool,
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
            char_surface_fatal: false,
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
        // The actual frame pacing is done at the tail of
        // `about_to_wait`. We keep this one-shot
        // `set_control_flow(Poll)` so the very first frame
        // kicks off without a synthetic `RedrawRequested` in
        // the cold-start path. (Winit 0.30 resets the control
        // flow to `Wait` after every `about_to_wait`, so this
        // initial value only affects the first wake-up.)
        event_loop.set_control_flow(ControlFlow::Poll);

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
                // PR-LX.2: wire the stand-alone Wayland input
                // region context. The winit window's raw display
                // / window handles resolve to a Wayland
                // connection only when the underlying compositor
                // is Wayland; on X11 / Windows this `try_new` is
                // a no-op and `state.wayland_region` stays
                // `None` (the dispatcher falls through to the
                // X11 / Windows path).
                #[cfg(target_os = "linux")]
                if self.state.wayland_region.is_none()
                    && let Some(ctx) =
                        crate::platform::wayland_region::WaylandInputRegionContext::try_new(
                            cw.window.as_ref(),
                        )
                {
                    self.state.wayland_region = Some(ctx);
                }

                // PR-LX.4: initialise the layer-shell detection
                // context. The actual probe runs lazily on the
                // first click-through dispatch (see
                // `apply_linux_click_through`).
                #[cfg(target_os = "linux")]
                if self.state.layer_shell.is_none() {
                    self.state.layer_shell =
                        Some(crate::platform::wayland_layer_shell::new_layer_shell_state());
                    // Run the probe eagerly so the first
                    // `apply_linux_click_through` log can carry
                    // the result. The probe is cheap (one
                    // registry round-trip) and cached.
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

                // PR-LX.5: open the X11 connection (if any)
                // for the click-through fallback. The probe
                // is cheap; on Wayland-only builds the
                // `RawDisplayHandle::X11` variant does not
                // match and `try_new` returns `None`. EWMH
                // `_NET_WM_STATE_SKIP_TASKBAR` +
                // `_NET_WM_STATE_SKIP_PAGER` are applied
                // once on construction so the character
                // window does not appear in the task
                // switcher.
                #[cfg(target_os = "linux")]
                if self.state.x11_ctx.is_none()
                    && let Some(ctx) =
                        crate::platform::x11_taskbar::X11Context::try_new(cw.window.as_ref())
                {
                    self.state.x11_ctx = Some(ctx);
                    tracing::info!(
                        target: "ene.linux.x11",
                        connected = true,
                        "X11 probe"
                    );
                }

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
                AppEvent::Tray(TrayAction::OpenSettings { page }) => {
                    self.show_settings_window();
                    // A.2: when the runtime triggers the open (e.g. on a
                    // `PermissionRequired` event), it can pass `Some(page)` to
                    // jump the tab strip straight to the AI page. The tray
                    // menu and click handlers always pass `None` (default =
                    // current page, falling back to Character on first show).
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
                        // A.2: jump to the AI page so the user
                        // immediately sees the permission dialog
                        // (data path wired in PR2; dialog rendering
                        // lands in A.5).
                        if let Some(uw) = self.ui_window.as_mut() {
                            uw.settings_ui.show(crate::settings_ui::PageKind::Ai);
                        }
                    }
                    crate::events::AiStreamUpdate::UserInputRequired { request_id, prompt } => {
                        pending_user_input = Some(PendingUserInput { request_id, prompt });
                        self.state.ui_state_mut().settings_window_visible = true;
                        // A.2: same as above — open to the AI page
                        // so the user sees the question dialog
                        // (rendering in A.5).
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

        // PR5.1: per-frame click-through. `device_query` gives us
        // the global cursor in screen pixels regardless of which
        // window currently owns focus; combined with the
        // character window's outer position this yields the
        // window-local cursor even when the window is currently
        // click-through (and so is not receiving `CursorMoved`
        // events from winit). The hit test is a BVH-backed Rapier
        // raycast against the character trimesh, not a single
        // AABB cuboid.
        //
        // PR5.6: the latest hit is stashed in `last_raycast_hit`
        // so the debug overlay (F3 / settings checkbox) can
        // highlight the hit collider and draw the hit-point
        // cross.
        if let Some(cw) = self.char_window.as_ref() {
            self.state.last_raycast_hit = update_char_window_cursor_and_hittest(
                &self.state,
                &self.device_state,
                cw,
                self.transparent,
                self.state.character.drag.is_dragging(),
                &mut self.last_cursor_physical,
            );

            let hovered_name = if let Some(hit) = self.state.last_raycast_hit
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
            self.state.ui_state_mut().hovered_bone_name = hovered_name;
        }

        // 4. Render both windows directly. Driving the
        //    `request_redraw` → `RedrawRequested` → render path
        //    is broken under winit 0.30.13's multi-window +
        //    `WaitUntil` bug (see PR5.7 in
        //    `docs/architecture/wgpu-migration.md`), so the
        //    character render is now invoked from
        //    `about_to_wait` instead. The UI window already
        //    rendered here for the same reason; we're aligning
        //    the char window to the same pattern.
        if let Err(AcquireError::Fatal) = self.render_char_frame() {
            // `render_char_frame` already logged and set
            // `char_surface_fatal`; the pacer below will
            // call `event_loop.exit()`.
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

        // 5. Frame pacing. `target_fps == 0` is the
        //    "Unlimited" choice in the settings UI; map it
        //    to `ControlFlow::Poll` (the swap chain's vsync
        //    is the only throttle). Otherwise sleep until
        //    the next frame deadline — the deadline is
        //    anchored to `last_frame_instant` (updated at
        //    the top of `render_char_frame`) with a `now()`
        //    fallback for the very first frame, so the
        //    elapsed `dt_secs` used by `update_motion` lines
        //    up with the chosen rate. PR5.7: this works
        //    because the char render is no longer driven by
        //    `RedrawRequested` (see the comment above); the
        //    earlier `WaitUntil(deadline)` pacer was blocked
        //    by the winit 0.30.13 multi-window bug, which
        //    this redesign sidesteps entirely.
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
                // A.7: rebuild the FXAA post-processor to
                // match the new swapchain size. The lazy
                // build in `character.render` is also keyed
                // on the size, but rebuilding here avoids
                // a single-frame miss when the user resizes
                // the character window while FXAA is on.
                character.resize_post_processor(
                    &gpu.device,
                    &gpu.queue,
                    (new_size.width, new_size.height),
                );
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
                    // A.8: round the integrated position to
                    // 0.01 world units (1 cm at the default
                    // ortho viewport). The legacy Bevy code
                    // had no rounding; on a high-resolution
                    // mouse this produced single-pixel
                    // sub-pixel jitter because the world
                    // coordinates stored more precision than
                    // the renderer can actually draw.
                    settings.character_state.character_position += delta;
                    settings.character_state.character_position.x =
                        (settings.character_state.character_position.x * 100.0).round() / 100.0;
                    settings.character_state.character_position.y =
                        (settings.character_state.character_position.y * 100.0).round() / 100.0;
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
                    .is_some_and(|hit| hit.entity == self.state.character_entity);
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
                    } else if key_code_pressed(&event) == Some(winit::keyboard::KeyCode::F3) {
                        // PR5.6: F3 toggles the per-bone
                        // collider wireframe + raycast
                        // hit-point overlay. Stays OFF across
                        // launches (no persistence).
                        let mut ui_state = self.state.ui_state_mut();
                        ui_state.show_collider_debug = !ui_state.show_collider_debug;
                        cw.window.request_redraw();
                    } else if key_code_pressed(&event) == Some(winit::keyboard::KeyCode::F8) {
                        // PR-LX.4: F8 toggles the "freeze
                        // character window" flag. The xdg-shell
                        // fallback uses this to force the
                        // character window to receive all input
                        // regardless of the cursor position. On
                        // Windows / X11 the flag is a no-op.
                        #[cfg(target_os = "linux")]
                        {
                            self.state.layer_shell_freeze = !self.state.layer_shell_freeze;
                            tracing::info!(
                                target: "ene.linux.layer_shell",
                                freeze = self.state.layer_shell_freeze,
                                "char window freeze toggled"
                            );
                        }
                        cw.window.request_redraw();
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
                // PR5.7: rendering is now driven directly from
                // `about_to_wait` (`render_char_frame`), so this
                // handler is a no-op. The `request_redraw` calls
                // sprinkled through the rest of this file
                // (e.g. resize, cursor-moved) are kept as
                // defensive wake-ups; they will simply enqueue
                // a no-op redraw that the next about_to_wait
                // overwrites.
            }
            _ => {}
        }
    }

    /// PR5.7: render the character window directly from
    /// `about_to_wait`, sidestepping `Window::request_redraw` and
    /// the `RedrawRequested` event path. This was originally
    /// done in `handle_char_window_event` but winit 0.30.13 has a
    /// bug where multi-window + `ControlFlow::WaitUntil` mode
    /// never delivers `RedrawRequested` to the first window
    /// created in the process — so the char window would freeze
    /// whenever the settings window was open. Driving the render
    /// from `about_to_wait` (where the UI window's render is
    /// already driven) makes the char render independent of
    /// `RedrawRequested` delivery, the winit bug becomes
    /// irrelevant, and we can restore the `WaitUntil(deadline)`
    /// frame pacer to save the CPU core that the
    /// `ControlFlow::Poll` workaround was burning.
    ///
    /// Returns `Err(AcquireError::Fatal)` only on a fatal
    /// surface error; `Reconfigure` and `Timeout` are handled
    /// inline (reconfigure the surface, or just retry next
    /// frame) and return `Ok(())`.
    fn render_char_frame(&mut self) -> Result<(), AcquireError> {
        let Some(cw) = self.char_window.as_mut() else {
            return Ok(());
        };
        let transparent = self.transparent;
        // PR5.6: snapshot the bits of state the debug overlay
        // needs before we destructure `self.state` (the
        // `&mut character` borrow would otherwise conflict
        // with the `ui_state()` borrow).
        let show_collider_debug = self.state.ui_state().show_collider_debug;
        let last_raycast_hit = self.state.last_raycast_hit;
        let character_entity = self.state.character_entity;
        let AppState {
            ref mut character,
            ref mut physics,
            ref mut debug_renderer,
            ref gpu,
            ref settings,
            ..
        } = self.state;
        let (device, queue) = (&gpu.device, &gpu.queue);

        // PR4.2 follow-up: one-shot diagnostic so we can
        // see whether the char window size / surface config
        // / depth texture / camera aspect / model uniform
        // are all in agreement. Logs on the very first
        // rendered frame only.
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
            let view_proj = character.camera_view_proj_dbg();
            let vp_scale_x =
                (view_proj[0][0].powi(2) + view_proj[1][0].powi(2) + view_proj[2][0].powi(2))
                    .sqrt();
            let vp_scale_y =
                (view_proj[0][1].powi(2) + view_proj[1][1].powi(2) + view_proj[2][1].powi(2))
                    .sqrt();
            let vp_scale_z =
                (view_proj[0][2].powi(2) + view_proj[1][2].powi(2) + view_proj[2][2].powi(2))
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
            // A.7: pass the swapchain size + format + AA mode
            // to the renderer. The post-processor is rebuilt
            // lazily inside `character.render` when the AA
            // mode, swapchain size, or format changes; the
            // runtime does not need to track those.
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
            if show_collider_debug && let Some(depth_view) = character.depth_view() {
                let mut lines = Vec::new();
                let (hit_collider, hit_point) = match last_raycast_hit {
                    Some(h) if h.entity == character_entity => (Some(h.collider), Some(h.point)),
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
                                    + (parent_pos_raw - center) * normalize_scale * actual_scale;
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
        match result {
            Ok(()) => Ok(()),
            Err(AcquireError::Reconfigure) => {
                tracing::warn!("Character Surface acquire Outdated/Lost; reconfiguring");
                cw.reconfigure(device, cw.window.inner_size());
                Ok(())
            }
            Err(AcquireError::Timeout) => {
                // The surface was unavailable this frame
                // (e.g. minimised, occluded). The next
                // about_to_wait will retry automatically —
                // nothing to do.
                Ok(())
            }
            Err(AcquireError::Fatal) => {
                tracing::error!("Character Surface acquire failed fatally; exiting");
                // Mark the fatal so the pacer calls
                // `event_loop.exit()` once instead of
                // logging on every frame. The user gets one
                // error message and the process exits.
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
        // PR9: when the WASD hotkey actually switched the
        // character, push the new character's default
        // expression into the renderer's EmotionQueue. The
        // WASD path is gated on the Character page (the early
        // `return` above) so the queue is always present.
        if matches!(
            action,
            SettingsAction::PrevCharacter | SettingsAction::NextCharacter
        ) {
            let now_secs = uw.settings_ui.started_at.elapsed().as_secs_f64();
            let default_expression = settings.character_state.default_expression.clone();
            uw.settings_ui
                .emotion_queue
                .push(crate::character_state::EmotionCommand {
                    emotion: default_expression,
                    target_time: now_secs,
                    hold_secs: 4.0,
                    weight: 1.0,
                });
        }
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
/// Returns the latest `RaycastHit` (or `None` on a clean miss /
/// off-window cursor) so the runtime can stash it in
/// [`crate::state::AppState::last_raycast_hit`] for the PR5.6
/// debug overlay. The cursor position is also stored in
/// `last_cursor` for the press / drag state machine fed by
/// winit's `CursorMoved` / `MouseInput` events.
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
) -> Option<crate::physics::RaycastHit> {
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
        Err(_) => return None,
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

    let cursor_over = hit.is_some_and(|h| h.entity == state.character_entity);

    let allows_input = !transparent || cursor_over || drag_is_dragging;

    // Stash the latest window-local cursor for the press / drag
    // state machine (which is fed by winit's `CursorMoved` /
    // `MouseInput` events).
    *last_cursor = Some(winit::dpi::PhysicalPosition::new(
        local_physical_x,
        local_physical_y,
    ));

    // 4. OS-level click-through.
    //    Windows: winit toggles `WS_EX_TRANSPARENT` for us.
    //    Linux: `set_cursor_hittest` is a no-op; the display-server-
    //           specific dispatcher (Wayland `set_input_region` or
    //           X11 shape) lives in `platform::platform_runtime`.
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
            state.layer_shell_freeze,
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

/// Like [`key_pressed`] but for the raw physical key code.
/// Used for F3, which has no `NamedKey` variant in winit 0.30.
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
