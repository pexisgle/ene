//! Shared winit event loop for Experiments A and B.

use std::sync::Arc;
use std::time::{Duration, Instant};

use slint::ComponentHandle;
use winit::application::ApplicationHandler;
use winit::dpi::PhysicalSize;
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowId, WindowLevel};

use crate::PocError;
use crate::blit::UiBlit;
use crate::gpu::{self, GpuContext};
use crate::input::{
    PointerTarget, ScreenPoint, VrmHitLayout, interactive_rects, route_pointer,
    triangle_placeholder_layout,
};
use crate::metrics::{self, Metrics};
use crate::os_input::{NativeInputRegion, StageInputRegion};
use crate::slint_host::{self, SlintHost};
use crate::triangle::TrianglePass;
use crate::vrm_scene::VrmScene;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PocMode {
    Composition,
    Input,
    Baseline,
}

pub fn run(mode: PocMode) -> Result<(), PocError> {
    crate::init_tracing();
    match mode {
        PocMode::Baseline => run_baseline(),
        PocMode::Composition | PocMode::Input => run_slint(mode),
    }
}

fn run_slint(mode: PocMode) -> Result<(), PocError> {
    let event_loop = EventLoop::new().map_err(|err| PocError::Window(err.to_string()))?;
    let mut app = SlintApp {
        mode,
        inner: None,
        error: None,
    };
    event_loop
        .run_app(&mut app)
        .map_err(|err| PocError::Window(err.to_string()))?;
    if let Some(err) = app.error.take() {
        return Err(err);
    }
    Ok(())
}

struct SlintApp {
    mode: PocMode,
    inner: Option<Running>,
    error: Option<PocError>,
}

struct Running {
    mode: PocMode,
    gpu: GpuContext,
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    depth: wgpu::Texture,
    depth_view: wgpu::TextureView,
    ui_tex: wgpu::Texture,
    ui_view: wgpu::TextureView,
    triangle: TrianglePass,
    blit: UiBlit,
    vrm: Option<VrmScene>,
    host: SlintHost,
    input: NativeInputRegion,
    metrics: Metrics,
    cursor_logical: Option<slint::LogicalPosition>,
    last_target: PointerTarget,
    started: Instant,
    animating: bool,
    bench_until: Option<Instant>,
    shared_device: bool,
}

impl ApplicationHandler for SlintApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.inner.is_some() {
            return;
        }
        match Running::create(event_loop, self.mode) {
            Ok(running) => self.inner = Some(running),
            Err(err) => {
                tracing::error!(error = %err, "poc failed to start");
                self.error = Some(err);
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
        let Some(running) = self.inner.as_mut() else {
            return;
        };
        if running.window.id() != window_id {
            return;
        }
        if !running.on_event(event_loop, &event) {
            event_loop.exit();
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let Some(running) = self.inner.as_mut() else {
            return;
        };
        if running.tick_bench(event_loop) {
            running.window.request_redraw();
        }
        event_loop.set_control_flow(if running.animating {
            ControlFlow::Poll
        } else {
            ControlFlow::Wait
        });
    }
}

impl Running {
    fn create(event_loop: &ActiveEventLoop, mode: PocMode) -> Result<Self, PocError> {
        let gpu = pollster::block_on(GpuContext::create())?;
        let info = gpu.adapter.get_info();
        tracing::info!(
            backend = ?info.backend,
            adapter = %info.name,
            "GpuContext created (ene-stage equivalent)"
        );
        let attrs = Window::default_attributes()
            .with_title("ene-stage-poc")
            .with_inner_size(PhysicalSize::new(800, 600))
            .with_transparent(true)
            .with_decorations(false)
            .with_window_level(WindowLevel::AlwaysOnTop);
        #[cfg(target_os = "windows")]
        let attrs = {
            use winit::platform::windows::WindowAttributesExtWindows;
            attrs.with_no_redirection_bitmap(true)
        };
        let window = Arc::new(
            event_loop
                .create_window(attrs)
                .map_err(|err| PocError::Window(err.to_string()))?,
        );
        let surface = gpu.create_surface(Arc::clone(&window))?;
        let format = gpu.surface_format(&surface);
        let alpha = gpu::pick_alpha_mode(&surface, &gpu.adapter);
        let transparency = gpu::alpha_mode_supports_transparency(alpha);
        tracing::info!(
            format = ?format,
            alpha_mode = ?alpha,
            transparency,
            "surface configured"
        );
        let size = window.inner_size();
        let config = gpu::configure_surface(&surface, &gpu.device, format, size, alpha);
        let (depth, depth_view) = gpu::create_depth(&gpu.device, config.width, config.height);
        let (ui_tex, ui_view) = gpu::create_ui_target(&gpu.device, config.width, config.height);
        let triangle = TrianglePass::new(&gpu.device, format);
        let blit = UiBlit::new(&gpu.device, format);
        let vrm = match VrmScene::load(&gpu.device, &gpu.queue, format) {
            Ok(scene) => {
                tracing::info!(source = %scene.source(), "VRM scene loaded");
                Some(scene)
            }
            Err(err) => {
                tracing::warn!(error = %err, "VRM unavailable; using triangle");
                None
            }
        };
        let handles = gpu.handles();
        let host = slint_host::install(Arc::clone(&window), handles)?;
        tracing::info!(
            "FemtoVGWGPURenderer constructed from cloned GpuContext instance/device/queue"
        );
        let ui_weak = host.ui.as_weak();
        host.ui.on_bubble_clicked(move || {
            let clicks = ui_weak.upgrade().map_or(0, |ui| ui.get_click_count());
            tracing::info!(
                ui_hit = true,
                vrm_hit = false,
                passthrough = false,
                target = "ui",
                clicks,
                "bubble clicked"
            );
        });
        let input = NativeInputRegion::attach(Arc::clone(&window));
        let bench_secs = std::env::var("ENE_STAGE_POC_SECONDS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .filter(|secs| *secs > 0);
        let started = Instant::now();
        let animating = false;
        window.request_redraw();
        Ok(Self {
            mode,
            gpu,
            window,
            surface,
            config,
            depth,
            depth_view,
            ui_tex,
            ui_view,
            triangle,
            blit,
            vrm,
            host,
            input,
            metrics: Metrics::start("idle"),
            cursor_logical: None,
            last_target: PointerTarget::Passthrough,
            started,
            animating,
            bench_until: bench_secs.map(|secs| started + Duration::from_secs(secs)),
            shared_device: true,
        })
    }

    fn on_event(&mut self, event_loop: &ActiveEventLoop, event: &WindowEvent) -> bool {
        match event {
            WindowEvent::Resized(size) => {
                self.resize(*size);
                self.window.request_redraw();
            }
            WindowEvent::ScaleFactorChanged { .. } => {
                self.host.adapter.set_size(
                    self.window.inner_size().width,
                    self.window.inner_size().height,
                );
                self.window.request_redraw();
            }
            WindowEvent::RedrawRequested => {
                if let Err(err) = self.render_frame() {
                    tracing::warn!(error = %err, "frame failed");
                }
            }
            WindowEvent::KeyboardInput { event, .. }
                if event.state == ElementState::Pressed
                    && (event.logical_key == Key::Named(NamedKey::Escape)
                        || event.logical_key == Key::Character("q".into())) =>
            {
                self.finish();
                event_loop.exit();
                return false;
            }
            WindowEvent::CloseRequested => {
                self.finish();
                return false;
            }
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } => {
                self.log_click();
            }
            _ => {}
        }
        let scale = self.window.scale_factor();
        slint_host::dispatch_winit_event(&self.host.ui, event, scale, &mut self.cursor_logical);
        slint::platform::update_timers_and_animations();
        true
    }

    fn tick_bench(&mut self, event_loop: &ActiveEventLoop) -> bool {
        let elapsed = self.started.elapsed();
        if self.mode == PocMode::Composition && elapsed >= Duration::from_secs(3) && !self.animating
        {
            self.metrics.rotate_phase("animation");
            self.animating = true;
        }
        if self.animating {
            let wave = self.started.elapsed().as_secs_f32();
            self.host.ui.set_bubble_y(80.0 + wave.sin() * 24.0);
        }
        if let Some(deadline) = self.bench_until
            && Instant::now() >= deadline
        {
            self.finish();
            event_loop.exit();
            return false;
        }
        self.animating || self.host.ui.window().has_active_animations()
    }

    fn resize(&mut self, size: PhysicalSize<u32>) {
        if size.width == 0 || size.height == 0 {
            return;
        }
        self.config.width = size.width;
        self.config.height = size.height;
        self.surface.configure(&self.gpu.device, &self.config);
        let (depth, depth_view) = gpu::create_depth(&self.gpu.device, size.width, size.height);
        self.depth = depth;
        self.depth_view = depth_view;
        let (ui_tex, ui_view) = gpu::create_ui_target(&self.gpu.device, size.width, size.height);
        self.ui_tex = ui_tex;
        self.ui_view = ui_view;
        self.host.adapter.set_size(size.width, size.height);
    }

    fn render_frame(&mut self) -> Result<(), PocError> {
        slint::platform::update_timers_and_animations();
        let scale = self.window.scale_factor() as f32;
        let size = self.window.inner_size();
        if self.config.width != size.width || self.config.height != size.height {
            self.resize(size);
        }
        let frame = gpu::acquire_frame(&self.surface).map_err(PocError::Surface)?;
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("ene-stage-poc.frame"),
            });
        if let Some(vrm) = self.vrm.as_mut() {
            vrm.render(
                &self.gpu.queue,
                &mut encoder,
                &view,
                &self.depth_view,
                self.config.width,
                self.config.height,
                true,
            );
        } else {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("ene-stage-poc.3d"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                occlusion_query_set: None,
                timestamp_writes: None,
                multiview_mask: None,
            });
            self.triangle.draw(&mut pass);
        }
        self.gpu.queue.submit(std::iter::once(encoder.finish()));

        self.host
            .adapter
            .render_ui(&self.ui_view, self.config.width, self.config.height)?;

        let mut encoder = self
            .gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("ene-stage-poc.ui-blit"),
            });
        self.blit
            .draw(&self.gpu.device, &mut encoder, &self.ui_view, &view);
        self.gpu.queue.submit(std::iter::once(encoder.finish()));
        frame.present();
        self.metrics.on_frame();

        let layout = self.vrm_layout();
        let ui_rect = slint_host::bubble_rect(&self.host.ui, scale);
        self.input
            .update_input_region(&interactive_rects(&[ui_rect], Some(&layout)));
        self.update_status(scale);
        Ok(())
    }

    fn vrm_layout(&self) -> VrmHitLayout {
        let viewport = (self.config.width, self.config.height);
        self.vrm.as_ref().map_or_else(
            || triangle_placeholder_layout(viewport),
            |scene| scene.hit_layout(viewport),
        )
    }

    fn log_click(&mut self) {
        let scale = self.window.scale_factor();
        let Some(pos) = self.cursor_logical else {
            return;
        };
        let cursor = ScreenPoint {
            x: pos.x * scale as f32,
            y: pos.y * scale as f32,
        };
        let ui_rect = slint_host::bubble_rect(&self.host.ui, scale as f32);
        let layout = self.vrm_layout();
        let target = route_pointer(cursor, &[ui_rect], Some(&layout));
        self.last_target = target;
        let (ui_hit, vrm_hit, passthrough) = match target {
            PointerTarget::Ui => (true, false, false),
            PointerTarget::Vrm(_) => (false, true, false),
            PointerTarget::Passthrough => (false, false, true),
        };
        println!(
            "UI hit: {ui_hit}  VRM hit: {vrm_hit:?}  passthrough: {passthrough}  target: {target:?}"
        );
        tracing::info!(ui_hit, vrm_hit, passthrough, ?target, "pointer route");
    }

    fn update_status(&self, scale: f32) {
        let info = self.gpu.adapter.get_info();
        let scene = self
            .vrm
            .as_ref()
            .map_or_else(|| "triangle".to_owned(), |scene| scene.source().to_owned());
        let text = format!(
            "{mode:?}  {scene}  shared wgpu  scale={scale:.2}  {server}  last={target:?}",
            mode = self.mode,
            server = self.input.display_server().name(),
            target = self.last_target,
        );
        let _ = info;
        self.host.ui.set_status_text(text.into());
    }

    fn finish(&mut self) {
        let info = self.gpu.adapter.get_info();
        let reports = self.metrics.reports();
        let extra = format!(
            "shared_device={shared} transparency={alpha} vrm={vrm} input={server} partial_region={partial} zero_copy=gpu-texture-blit",
            shared = self.shared_device,
            alpha = gpu::alpha_mode_supports_transparency(self.config.alpha_mode),
            vrm = self.vrm.is_some(),
            server = self.input.display_server().name(),
            partial = self.input.supports_partial_region(),
        );
        metrics::print_reports(
            match self.mode {
                PocMode::Composition => "experiment-a",
                PocMode::Input => "experiment-b",
                PocMode::Baseline => "baseline",
            },
            &info.name,
            &format!("{:?}", info.backend),
            &extra,
            &reports,
        );
    }
}

fn run_baseline() -> Result<(), PocError> {
    let event_loop = EventLoop::new().map_err(|err| PocError::Window(err.to_string()))?;
    let mut app = BaselineApp {
        inner: None,
        error: None,
    };
    event_loop
        .run_app(&mut app)
        .map_err(|err| PocError::Window(err.to_string()))?;
    if let Some(err) = app.error.take() {
        return Err(err);
    }
    Ok(())
}

struct BaselineApp {
    inner: Option<BaselineRunning>,
    error: Option<PocError>,
}

struct BaselineRunning {
    gpu: GpuContext,
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    depth: wgpu::Texture,
    depth_view: wgpu::TextureView,
    triangle: TrianglePass,
    egui_ctx: egui::Context,
    egui_state: egui_winit::State,
    renderer: egui_wgpu::Renderer,
    metrics: Metrics,
    started: Instant,
    animating: bool,
    bench_until: Option<Instant>,
}

impl ApplicationHandler for BaselineApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.inner.is_some() {
            return;
        }
        match BaselineRunning::create(event_loop) {
            Ok(inner) => self.inner = Some(inner),
            Err(err) => {
                self.error = Some(err);
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
        let Some(inner) = self.inner.as_mut() else {
            return;
        };
        if inner.window.id() != window_id {
            return;
        }
        if inner
            .egui_state
            .on_window_event(&inner.window, &event)
            .repaint
        {
            inner.window.request_redraw();
        }
        match event {
            WindowEvent::Resized(size) => inner.resize(size),
            WindowEvent::RedrawRequested => {
                if let Err(err) = inner.render() {
                    tracing::warn!(error = %err, "baseline frame failed");
                }
            }
            WindowEvent::CloseRequested => {
                inner.finish();
                event_loop.exit();
            }
            WindowEvent::KeyboardInput { event: ref key, .. }
                if key.state == ElementState::Pressed
                    && matches!(key.logical_key, Key::Named(NamedKey::Escape)) =>
            {
                inner.finish();
                event_loop.exit();
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let Some(inner) = self.inner.as_mut() else {
            return;
        };
        let elapsed = inner.started.elapsed();
        if elapsed >= Duration::from_secs(3) && !inner.animating {
            inner.metrics.rotate_phase("animation");
            inner.animating = true;
        }
        if let Some(deadline) = inner.bench_until
            && Instant::now() >= deadline
        {
            inner.finish();
            event_loop.exit();
            return;
        }
        inner.window.request_redraw();
        event_loop.set_control_flow(if inner.animating {
            ControlFlow::Poll
        } else {
            ControlFlow::Wait
        });
    }
}

impl BaselineRunning {
    fn create(event_loop: &ActiveEventLoop) -> Result<Self, PocError> {
        let gpu = pollster::block_on(GpuContext::create())?;
        let attrs = Window::default_attributes()
            .with_title("ene-stage-poc-baseline")
            .with_inner_size(PhysicalSize::new(800, 600))
            .with_transparent(true)
            .with_decorations(false)
            .with_window_level(WindowLevel::AlwaysOnTop);
        let window = Arc::new(
            event_loop
                .create_window(attrs)
                .map_err(|err| PocError::Window(err.to_string()))?,
        );
        let surface = gpu.create_surface(Arc::clone(&window))?;
        let format = gpu.surface_format(&surface);
        let alpha = gpu::pick_alpha_mode(&surface, &gpu.adapter);
        let config =
            gpu::configure_surface(&surface, &gpu.device, format, window.inner_size(), alpha);
        let (depth, depth_view) = gpu::create_depth(&gpu.device, config.width, config.height);
        let triangle = TrianglePass::new(&gpu.device, format);
        let egui_ctx = egui::Context::default();
        let egui_state = egui_winit::State::new(
            egui_ctx.clone(),
            egui::ViewportId::ROOT,
            window.as_ref(),
            Some(window.scale_factor() as f32),
            None,
            None,
        );
        let renderer = egui_wgpu::Renderer::new(
            &gpu.device,
            format,
            egui_wgpu::RendererOptions {
                msaa_samples: 1,
                depth_stencil_format: None,
                dithering: true,
                predictable_texture_filtering: false,
            },
        );
        let bench_secs = std::env::var("ENE_STAGE_POC_SECONDS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .filter(|secs| *secs > 0);
        let started = Instant::now();
        window.request_redraw();
        Ok(Self {
            gpu,
            window,
            surface,
            config,
            depth,
            depth_view,
            triangle,
            egui_ctx,
            egui_state,
            renderer,
            metrics: Metrics::start("idle"),
            started,
            animating: false,
            bench_until: bench_secs.map(|secs| started + Duration::from_secs(secs)),
        })
    }

    fn resize(&mut self, size: PhysicalSize<u32>) {
        if size.width == 0 || size.height == 0 {
            return;
        }
        self.config.width = size.width;
        self.config.height = size.height;
        self.surface.configure(&self.gpu.device, &self.config);
        let (depth, depth_view) = gpu::create_depth(&self.gpu.device, size.width, size.height);
        self.depth = depth;
        self.depth_view = depth_view;
    }

    fn render(&mut self) -> Result<(), PocError> {
        let frame = gpu::acquire_frame(&self.surface).map_err(PocError::Surface)?;
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("ene-stage-poc.baseline"),
            });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("ene-stage-poc.baseline.3d"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                occlusion_query_set: None,
                timestamp_writes: None,
                multiview_mask: None,
            });
            self.triangle.draw(&mut pass);
        }
        let raw = self.egui_state.take_egui_input(&self.window);
        let wave = self.started.elapsed().as_secs_f32();
        let y = if self.animating {
            80.0 + wave.sin() * 24.0
        } else {
            80.0
        };
        let full = self.egui_ctx.run_ui(raw, |ui| {
            egui::Area::new(egui::Id::new("bubble"))
                .fixed_pos(egui::pos2(40.0, y))
                .show(ui, |ui| {
                    egui::Frame::NONE
                        .fill(egui::Color32::from_rgba_unmultiplied(20, 32, 51, 204))
                        .corner_radius(18.0)
                        .inner_margin(12.0)
                        .show(ui, |ui| {
                            ui.label("egui overlay");
                            if ui.button("Tap the bubble").clicked() {
                                tracing::info!(target = "ui", "egui bubble clicked");
                            }
                        });
                });
        });
        self.egui_state
            .handle_platform_output(&self.window, full.platform_output);
        let pixels_per_point = self.window.scale_factor() as f32;
        let screen = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [self.config.width.max(1), self.config.height.max(1)],
            pixels_per_point,
        };
        let primitives = self.egui_ctx.tessellate(full.shapes, pixels_per_point);
        for (id, delta) in &full.textures_delta.set {
            self.renderer
                .update_texture(&self.gpu.device, &self.gpu.queue, *id, delta);
        }
        let extra = self.renderer.update_buffers(
            &self.gpu.device,
            &self.gpu.queue,
            &mut encoder,
            &primitives,
            &screen,
        );
        {
            let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("ene-stage-poc.baseline.ui"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: None,
                occlusion_query_set: None,
                timestamp_writes: None,
                multiview_mask: None,
            });
            self.renderer
                .render(&mut pass.forget_lifetime(), &primitives, &screen);
        }
        for id in &full.textures_delta.free {
            self.renderer.free_texture(id);
        }
        self.gpu
            .queue
            .submit(extra.into_iter().chain(std::iter::once(encoder.finish())));
        frame.present();
        self.metrics.on_frame();
        Ok(())
    }

    fn finish(&mut self) {
        let info = self.gpu.adapter.get_info();
        let reports = self.metrics.reports();
        metrics::print_reports(
            "egui-baseline",
            &info.name,
            &format!("{:?}", info.backend),
            "transparent wgpu + egui chrome on the same device/queue",
            &reports,
        );
    }
}
