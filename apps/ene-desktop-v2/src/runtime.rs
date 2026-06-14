//! Winit event loop driver.
//!
//! Owns the winit window(s), their wgpu surfaces, and the
//! [`AppState`]. Implements winit 0.30's `ApplicationHandler` so
//! `EventLoop::run_app` is a single call from `main`.
//!
//! Each frame:
//!
//! 1. `about_to_wait` drains the cross-subsystem [`AppEvent`] bus,
//!    forwarding tray actions to UI state and pushing AI events
//!    into the bridge's inbox. It also ticks the GTK pump on
//!    Linux and calls `flush_if_dirty` on the settings.
//! 2. `window_event` dispatches input. Keyboard `Space` toggles
//!    transparency (PR0 smoke test), `Escape` quits, the window
//!    close button quits. The egui UI window consumes
//!    `WindowEvent`s via its `egui_winit::State`.
//! 3. `RedrawRequested` renders the current frame to each surface.
use std::sync::Arc;
use wgpu::util::DeviceExt;
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalSize};
use winit::event::{ElementState, KeyEvent, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowAttributes, WindowId, WindowLevel};

use crate::events::{AppEvent, AppEventSender, TrayAction};
use crate::gpu::pick_format_and_alpha;
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

        // Create the UI window
        let ui_attrs = WindowAttributes::default()
            .with_title("ene UI")
            .with_inner_size(LogicalSize::new(400.0, 300.0))
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
                uw.window.request_redraw();
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
            if let Some(uw) = self.ui_window.as_mut() {
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
                        event_loop.exit();
                    }
                    WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. } => {
                        uw.reconfigure(&self.state.gpu.device, uw.window.inner_size());
                        uw.window.request_redraw();
                    }
                    WindowEvent::RedrawRequested => {
                        // Rendering happens in `about_to_wait` to
                        // avoid winit 0.30's `RedrawRequested`
                        // double-fire on Windows.
                    }
                    _ => {}
                }
            }
        } else if is_char {
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
                WindowEvent::KeyboardInput { .. } => match key_pressed(&event) {
                    Some(NamedKey::Space) => {
                        self.transparent = !self.transparent;
                        cw.window.set_decorations(!self.transparent);
                        cw.window.request_redraw();
                    }
                    Some(NamedKey::Escape) => {
                        self.state.save();
                        event_loop.exit();
                    }
                    _ => {}
                },
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
                                tracing::error!(
                                    "Character Surface acquire failed fatally; exiting"
                                );
                                event_loop.exit();
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        // Drain the cross-subsystem bus. Tray actions update UI
        // state; AI events are absorbed by the bridge. `Quit` exits.
        while let Ok(event) = self.state.event_rx.try_recv() {
            match event {
                AppEvent::Tray(TrayAction::OpenSettings) => {
                    self.state.settings.ui.settings_window_visible = true;
                    if let Some(uw) = self.ui_window.as_ref() {
                        uw.window.request_redraw();
                    }
                }
                AppEvent::Tray(TrayAction::Quit) => {
                    self.state.save();
                    event_loop.exit();
                }
                AppEvent::Quit => {
                    self.state.save();
                    event_loop.exit();
                }
                AppEvent::Ai(_update) => {
                    // Drain holds these for the UI; the bridge has
                    // its own inbox buffer.
                }
                AppEvent::EmoteToken(token) => {
                    tracing::info!("[Ene] emotion token: {token}");
                }
            }
        }
        // Push remaining AI events into the bridge inbox for the
        // UI pass to read.
        self.state.ai.drain(&mut self.state.event_rx);

        // Auto-save any settings that were marked dirty by UI
        // handlers in this frame.
        self.state.settings.flush_if_dirty();

        // Pump GTK on Linux so the tray stays alive.
        if let Some(_tray) = self.state.tray.as_ref() {
            #[cfg(target_os = "linux")]
            _tray.tick_gtk();
        }

        // Trigger a redraw for both windows.
        if let Some(cw) = self.char_window.as_mut() {
            cw.window.request_redraw();
        }
        if let Some(uw) = self.ui_window.as_mut() {
            match uw.render_frame(&self.state.gpu.device, &self.state.gpu.queue) {
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
    text_input: String,
    click_count: u32,
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
            text_input: String::new(),
            click_count: 0,
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
            egui::Id::new("central_panel"),
            egui::UiBuilder::new()
                .layer_id(egui::LayerId::background())
                .max_rect(self.egui_ctx.content_rect()),
        );
        panel_ui.set_clip_rect(self.egui_ctx.content_rect());

        egui::CentralPanel::default().show_inside(&mut panel_ui, |ui| {
            ui.heading("ene UI Settings");
            ui.label("Hello from separate egui window!");

            ui.separator();
            ui.horizontal_top(|ui| {
                ui.label("Name: ");
                ui.add_sized(
                    [300.0, ui.spacing().interact_size.y],
                    egui::TextEdit::singleline(&mut self.text_input),
                );
            });

            if ui.button("Click me!").clicked() {
                self.click_count += 1;
            }
            ui.label(format!("Button clicked {} times", self.click_count));
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

// `CharacterSettings` is the only import we expose to keep
// `clippy --fix` quiet about unused imports while the smoke UI
// does not yet reach into the struct.
#[allow(unused_imports)]
use crate::settings::CharacterSettings as _CharacterSettings;
