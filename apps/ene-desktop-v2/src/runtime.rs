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
use wgpu::util::DeviceExt;
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalSize};
use winit::event::{ElementState, KeyEvent, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowAttributes, WindowId, WindowLevel};

use crate::ai_bridge::AiBridge;
use crate::events::{AppEvent, AppEventSender, TrayAction};
use crate::gpu::pick_format_and_alpha;
use crate::settings::{CharacterSettings, PendingPermission, PendingUserInput, QuestionDraft};
use crate::settings_ui::{PageKind, SettingsUi};
use crate::state::AppState;

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
    rect: Option<RectRenderer>,
}

impl Runtime {
    pub fn new(state: AppState, event_tx: AppEventSender) -> Self {
        Self {
            state,
            event_tx,
            transparent: false,
            char_window: None,
            ui_window: None,
            rect: None,
        }
    }

    fn show_settings_window(&mut self) {
        if let Some(uw) = self.ui_window.as_mut()
            && !self.state.settings.ui.settings_window_visible
        {
            self.state.settings.ui.settings_window_visible = true;
            uw.settings_ui.sync_from_settings(&self.state.settings);
            uw.window.set_visible(true);
            uw.window.request_redraw();
        }
    }

    fn hide_settings_window(&mut self) {
        if self.state.settings.ui.settings_window_visible {
            self.state.save();
            self.state.settings.ui.settings_window_visible = false;
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
                self.rect = Some(RectRenderer::new(&self.state.gpu.device, format));
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
                uw.window
                    .set_visible(self.state.settings.ui.settings_window_visible);
                if self.state.settings.ui.settings_window_visible {
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
                        self.state.settings.ui.ai_latest_response.push_str(&text);
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
                        self.state.settings.ui.settings_window_visible = true;
                    }
                    crate::events::AiStreamUpdate::UserInputRequired { request_id, prompt } => {
                        pending_user_input = Some(PendingUserInput { request_id, prompt });
                        self.state.settings.ui.settings_window_visible = true;
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
                            });
                    }
                }
            }
        }
        if let Some(perm) = pending_permission {
            self.state.settings.ui.pending_permission = Some(perm);
        }
        if let Some(pui) = pending_user_input {
            self.state.settings.ui.user_input_drafts = (0..pui.prompt.items.len())
                .map(|_| QuestionDraft::default())
                .collect();
            self.state.settings.ui.pending_user_input = Some(pui);
        }

        // 2. Auto-save any settings that were marked dirty by UI
        //    handlers in this frame.
        self.state.settings.flush_if_dirty();

        // 3. Pump GTK on Linux so the tray stays alive.
        if let Some(_tray) = self.state.tray.as_ref() {
            #[cfg(target_os = "linux")]
            _tray.tick_gtk();
        }

        // 4. Trigger a redraw for both windows.
        if let Some(cw) = self.char_window.as_mut() {
            cw.window.request_redraw();
        }
        if let Some(uw) = self.ui_window.as_mut() {
            if uw.window.is_visible() != Some(self.state.settings.ui.settings_window_visible) {
                uw.window
                    .set_visible(self.state.settings.ui.settings_window_visible);
            }
            if self.state.settings.ui.settings_window_visible {
                uw.window.request_redraw();
                match uw.render_frame(
                    &self.state.gpu.device,
                    &self.state.gpu.queue,
                    &mut self.state.settings,
                    &self.state.ai,
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
        let cw = self.char_window.as_mut().unwrap();
        match event {
            WindowEvent::CloseRequested => {
                self.state.save();
                event_loop.exit();
            }
            WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. } => {
                cw.reconfigure(&self.state.gpu.device, cw.window.inner_size());
                cw.window.request_redraw();
            }
            WindowEvent::KeyboardInput { .. } => {
                if let Some(named) = key_pressed(&event) {
                    if matches!(named, NamedKey::Space) {
                        self.transparent = !self.transparent;
                        cw.window.set_decorations(!self.transparent);
                        cw.window.request_redraw();
                    } else if matches!(named, NamedKey::Escape) {
                        self.state.save();
                        event_loop.exit();
                    } else if matches!(named, NamedKey::F1) {
                        if self.state.settings.ui.settings_window_visible {
                            self.hide_settings_window();
                        } else {
                            self.show_settings_window();
                        }
                    } else {
                        // Character-window WASD / Space shortcuts
                        // when the settings window is open on the
                        // Character page and egui is not focused.
                        if self.state.settings.ui.settings_window_visible
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
                let Some(rect) = self.rect.as_ref() else {
                    return;
                };
                if let Err(e) = cw.render_frame(
                    &self.state.gpu.device,
                    &self.state.gpu.queue,
                    rect,
                    self.transparent,
                ) {
                    match e {
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
                self.state.settings.ui.settings_window_visible = false;
            }
            WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. } => {
                uw.reconfigure(&self.state.gpu.device, uw.window.inner_size());
                uw.window.request_redraw();
            }
            WindowEvent::KeyboardInput { .. } => {
                if let Some(NamedKey::Escape) = key_pressed(&event) {
                    self.state.save();
                    self.state.settings.ui.settings_window_visible = false;
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
            ..
        } = self.state;
        crate::settings_ui::widgets::apply_action(
            action,
            settings,
            &mut uw.settings_ui.animation,
            ai,
        );
    }
}

fn cw_char_window_has_focus(cw: &CharacterWindow) -> bool {
    cw.window.has_focus()
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

    fn render_frame(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        rect: &RectRenderer,
        transparent: bool,
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

        let (gray, alpha) = if transparent { (0.0, 0.0) } else { (0.2, 1.0) };
        let clear_color = wgpu::Color {
            r: gray,
            g: gray,
            b: gray,
            a: alpha,
        };

        let mut encoder =
            device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        {
            let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: None,
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(clear_color),
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            rect.draw(&mut rp);
        }
        queue.submit(std::iter::once(encoder.finish()));
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
            self.settings_ui.render(ui, settings, ai);
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

struct RectRenderer {
    pipeline: wgpu::RenderPipeline,
    vbuf: wgpu::Buffer,
}

const QUAD: &[glam::Vec2] = &[
    glam::Vec2::new(-0.5, -0.5),
    glam::Vec2::new(0.5, -0.5),
    glam::Vec2::new(0.5, 0.5),
    glam::Vec2::new(-0.5, -0.5),
    glam::Vec2::new(0.5, 0.5),
    glam::Vec2::new(-0.5, 0.5),
];

const SHADER: &str = r#"
const COLOR: vec4<f32> = vec4(0.85, 0.20, 0.30, 1.0);

struct VsIn {
    @location(0) position: vec2<f32>,
};
struct VsOut {
    @builtin(position) clip_pos: vec4<f32>,
};

@vertex
fn vs_main(in: VsIn) -> VsOut {
    var out: VsOut;
    out.clip_pos = vec4<f32>(in.position, 0.0, 1.0);
    return out;
}

@fragment
fn fs_main(_in: VsOut) -> @location(0) vec4<f32> {
    return COLOR;
}
"#;

impl RectRenderer {
    fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: None,
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });

        let vbuf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: bytemuck::cast_slice(QUAD),
            usage: wgpu::BufferUsages::VERTEX,
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[],
            immediate_size: 0,
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: None,
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<glam::Vec2>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x2],
                }],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });

        Self { pipeline, vbuf }
    }

    fn draw(&self, rp: &mut wgpu::RenderPass<'_>) {
        rp.set_pipeline(&self.pipeline);
        rp.set_vertex_buffer(0, self.vbuf.slice(..));
        rp.draw(0..6, 0..1);
    }
}
