//! Experiment C: fair compositor cost comparison (C0–C4).

use std::sync::Arc;
use std::time::{Duration, Instant};

use winit::application::ApplicationHandler;
use winit::dpi::PhysicalSize;
use winit::event::{ElementState, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowId, WindowLevel};

use crate::PocError;
use crate::blit::UiBlit;
use crate::gpu::{self, GpuContext};
use crate::metrics::{self, Metrics};
use crate::slint_host::{self, SlintHost};
use crate::vrm_scene::VrmScene;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Case {
    C0,
    C1,
    C2,
    C3,
    C4,
}

impl Case {
    #[must_use]
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_uppercase().as_str() {
            "C0" | "0" => Some(Self::C0),
            "C1" | "1" => Some(Self::C1),
            "C2" | "2" => Some(Self::C2),
            "C3" | "3" => Some(Self::C3),
            "C4" | "4" => Some(Self::C4),
            other if other.starts_with("C0") => Some(Self::C0),
            other if other.starts_with("C1") => Some(Self::C1),
            other if other.starts_with("C2") => Some(Self::C2),
            other if other.starts_with("C3") => Some(Self::C3),
            other if other.starts_with("C4") => Some(Self::C4),
            _ => None,
        }
    }

    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::C0 => "C0-vrm-only",
            Self::C1 => "C1-compositor-empty-ui",
            Self::C2 => "C2-static-slint",
            Self::C3 => "C3-animated-slint",
            Self::C4 => "C4-vrm-egui",
        }
    }

    #[must_use]
    pub const fn uses_slint(self) -> bool {
        matches!(self, Self::C1 | Self::C2 | Self::C3)
    }

    #[must_use]
    pub const fn uses_egui(self) -> bool {
        matches!(self, Self::C4)
    }

    #[must_use]
    pub const fn composite(self) -> bool {
        matches!(self, Self::C1 | Self::C2 | Self::C3)
    }
}

pub fn run() -> Result<(), PocError> {
    crate::init_tracing();
    let arg = std::env::args()
        .nth(1)
        .or_else(|| std::env::var("ENE_STAGE_POC_CASE").ok());
    if arg.as_deref().is_none_or(|v| v == "all") {
        return run_all();
    }
    let Some(case) = arg.as_deref().and_then(Case::parse) else {
        return Err(PocError::Window(
            "usage: ene-stage-poc-c C0|C1|C2|C3|C4|all".to_owned(),
        ));
    };
    run_case(case)
}

fn run_all() -> Result<(), PocError> {
    let exe = std::env::current_exe().map_err(|err| PocError::Window(err.to_string()))?;
    for case in [Case::C0, Case::C1, Case::C2, Case::C3, Case::C4] {
        tracing::info!(case = case.name(), "spawn experiment C case");
        let status = std::process::Command::new(&exe)
            .arg(match case {
                Case::C0 => "C0",
                Case::C1 => "C1",
                Case::C2 => "C2",
                Case::C3 => "C3",
                Case::C4 => "C4",
            })
            .env(
                "ENE_STAGE_POC_CASE",
                match case {
                    Case::C0 => "C0",
                    Case::C1 => "C1",
                    Case::C2 => "C2",
                    Case::C3 => "C3",
                    Case::C4 => "C4",
                },
            )
            .status()
            .map_err(|err| PocError::Window(err.to_string()))?;
        if !status.success() {
            return Err(PocError::Window(format!(
                "{} exited {}",
                case.name(),
                status
            )));
        }
    }
    Ok(())
}

fn run_case(case: Case) -> Result<(), PocError> {
    let event_loop = EventLoop::new().map_err(|err| PocError::Window(err.to_string()))?;
    let mut app = BenchApp {
        case,
        inner: None,
        error: None,
        created: Instant::now(),
    };
    event_loop
        .run_app(&mut app)
        .map_err(|err| PocError::Window(err.to_string()))?;
    if let Some(err) = app.error.take() {
        return Err(err);
    }
    Ok(())
}

struct BenchApp {
    case: Case,
    inner: Option<Bench>,
    error: Option<PocError>,
    created: Instant,
}

struct Bench {
    case: Case,
    gpu: GpuContext,
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    depth: wgpu::Texture,
    depth_view: wgpu::TextureView,
    ui_tex: Option<(wgpu::Texture, wgpu::TextureView)>,
    blit: Option<UiBlit>,
    vrm: VrmScene,
    slint: Option<SlintHost>,
    egui_ctx: Option<egui::Context>,
    egui_state: Option<egui_winit::State>,
    egui_renderer: Option<egui_wgpu::Renderer>,
    metrics: Metrics,
    phase: Phase,
    process_start: Instant,
    startup: Option<Duration>,
    warmup_until: Instant,
    measure_until: Instant,
    idle_until: Instant,
    anim_t0: Instant,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Warmup,
    Measure,
    Idle,
}

struct Timing {
    warmup: Duration,
    measure: Duration,
    idle: Duration,
}

fn timing() -> Timing {
    let warmup = env_ms("ENE_STAGE_POC_WARMUP_MS", 5_000);
    let measure = env_ms("ENE_STAGE_POC_MEASURE_MS", 12_000);
    let idle = env_ms("ENE_STAGE_POC_IDLE_MS", 5_000);
    Timing {
        warmup,
        measure,
        idle,
    }
}

fn env_ms(name: &str, default_ms: u64) -> Duration {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .map_or(Duration::from_millis(default_ms), Duration::from_millis)
}

impl ApplicationHandler for BenchApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.inner.is_some() {
            return;
        }
        match Bench::create(event_loop, self.case, self.created) {
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
        if let Some(state) = inner.egui_state.as_mut() {
            let _handled = state.on_window_event(&inner.window, &event);
        }
        match event {
            WindowEvent::Resized(size) => inner.resize(size),
            WindowEvent::RedrawRequested => {
                if inner.phase == Phase::Idle {
                    return;
                }
                if let Err(err) = inner.render() {
                    tracing::warn!(error = %err, "experiment C frame failed");
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
        let now = Instant::now();
        match inner.phase {
            Phase::Warmup if now >= inner.warmup_until => {
                inner.metrics.rotate_phase("measure");
                inner.phase = Phase::Measure;
            }
            Phase::Measure if now >= inner.measure_until => {
                inner.metrics.rotate_phase("idle");
                inner.phase = Phase::Idle;
            }
            Phase::Idle if now >= inner.idle_until => {
                inner.finish();
                event_loop.exit();
                return;
            }
            _ => {}
        }
        let redraw = matches!(inner.phase, Phase::Warmup | Phase::Measure);
        if redraw {
            inner.window.request_redraw();
            event_loop.set_control_flow(ControlFlow::Poll);
        } else {
            event_loop.set_control_flow(ControlFlow::WaitUntil(inner.idle_until));
        }
    }
}

impl Bench {
    fn create(
        event_loop: &ActiveEventLoop,
        case: Case,
        process_start: Instant,
    ) -> Result<Self, PocError> {
        let gpu = pollster::block_on(GpuContext::create())?;
        let info = gpu.adapter.get_info();
        tracing::info!(
            case = case.name(),
            adapter = %info.name,
            backend = ?info.backend,
            driver = %info.driver,
            driver_info = %info.driver_info,
            "experiment C gpu"
        );
        let attrs = Window::default_attributes()
            .with_title(format!("ene-stage-poc-c {}", case.name()))
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
        let size = window.inner_size();
        let config = gpu::configure_surface(&surface, &gpu.device, format, size, alpha);
        let (depth, depth_view) = gpu::create_depth(&gpu.device, config.width, config.height);
        let vrm = VrmScene::load(&gpu.device, &gpu.queue, format).map_err(PocError::Window)?;
        tracing::info!(source = %vrm.source(), "shared VRM for experiment C");
        let (ui_tex, blit, slint) = if case.uses_slint() {
            let (tex, view) = gpu::create_ui_target(&gpu.device, config.width, config.height);
            let blit = UiBlit::new(&gpu.device, format);
            let host = slint_host::install(Arc::clone(&window), gpu.handles())?;
            host.ui.set_show_bubble(matches!(case, Case::C2 | Case::C3));
            host.ui.set_show_menu(false);
            host.ui.set_show_cursor(case == Case::C3);
            (Some((tex, view)), Some(blit), Some(host))
        } else {
            (None, None, None)
        };
        let (egui_ctx, egui_state, egui_renderer) = if case.uses_egui() {
            let ctx = egui::Context::default();
            let state = egui_winit::State::new(
                ctx.clone(),
                egui::ViewportId::ROOT,
                window.as_ref(),
                Some({
                    #[expect(
                        clippy::cast_possible_truncation,
                        reason = "window scale factors are small"
                    )]
                    {
                        window.scale_factor() as f32
                    }
                }),
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
            (Some(ctx), Some(state), Some(renderer))
        } else {
            (None, None, None)
        };
        let times = timing();
        let now = Instant::now();
        window.request_redraw();
        Ok(Self {
            case,
            gpu,
            window,
            surface,
            config,
            depth,
            depth_view,
            ui_tex,
            blit,
            vrm,
            slint,
            egui_ctx,
            egui_state,
            egui_renderer,
            metrics: Metrics::start("warmup"),
            phase: Phase::Warmup,
            process_start,
            startup: None,
            warmup_until: now + times.warmup,
            measure_until: now + times.warmup + times.measure,
            idle_until: now + times.warmup + times.measure + times.idle,
            anim_t0: now,
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
        if self.ui_tex.is_some() {
            self.ui_tex = Some(gpu::create_ui_target(
                &self.gpu.device,
                size.width,
                size.height,
            ));
        }
        if let Some(host) = self.slint.as_mut() {
            host.adapter.set_size(size.width, size.height);
        }
    }

    fn render(&mut self) -> Result<(), PocError> {
        if self.case == Case::C3
            && matches!(self.phase, Phase::Warmup | Phase::Measure)
            && let Some(host) = self.slint.as_mut()
        {
            let t = self.anim_t0.elapsed().as_secs_f32();
            host.ui.set_bubble_y(80.0 + t.sin() * 24.0);
            host.ui
                .set_bubble_opacity(0.72 + 0.28 * (0.5 + 0.5 * (t * 2.0).sin()));
            host.ui.set_bubble_scale(1.0 + 0.05 * (t * 3.0).sin());
            host.ui
                .set_cursor_opacity(if (t * 3.0).sin() > 0.0 { 1.0 } else { 0.15 });
        }
        let frame = gpu::acquire_frame(&self.surface).map_err(PocError::Surface)?;
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("ene-stage-poc.c.vrm"),
            });
        self.vrm.render(
            &self.gpu.queue,
            &mut encoder,
            &view,
            &self.depth_view,
            self.config.width,
            self.config.height,
            true,
        );
        self.gpu.queue.submit(std::iter::once(encoder.finish()));

        if self.case.uses_slint() {
            let host = self
                .slint
                .as_mut()
                .ok_or_else(|| PocError::Slint("missing host".to_owned()))?;
            slint::platform::update_timers_and_animations();
            let ui_view = &self
                .ui_tex
                .as_ref()
                .ok_or_else(|| PocError::Slint("missing ui target".to_owned()))?
                .1;
            host.adapter
                .render_ui(ui_view, self.config.width, self.config.height)?;
            let mut encoder =
                self.gpu
                    .device
                    .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                        label: Some("ene-stage-poc.c.composite"),
                    });
            let blit = self
                .blit
                .as_ref()
                .ok_or_else(|| PocError::Slint("missing blit".to_owned()))?;
            blit.draw(&self.gpu.device, &mut encoder, ui_view, &view);
            self.gpu.queue.submit(std::iter::once(encoder.finish()));
        } else if self.case.uses_egui() {
            self.render_egui(&view)?;
        }

        frame.present();
        if self.startup.is_none() {
            let elapsed = self.process_start.elapsed();
            self.startup = Some(elapsed);
            tracing::info!(startup_ms = elapsed.as_secs_f64() * 1000.0, "first present");
        }
        if matches!(self.phase, Phase::Warmup | Phase::Measure) {
            self.metrics.on_frame();
        }
        Ok(())
    }

    fn render_egui(&mut self, view: &wgpu::TextureView) -> Result<(), PocError> {
        let ctx = self
            .egui_ctx
            .as_ref()
            .ok_or_else(|| PocError::Window("missing egui".to_owned()))?;
        let state = self
            .egui_state
            .as_mut()
            .ok_or_else(|| PocError::Window("missing egui state".to_owned()))?;
        let renderer = self
            .egui_renderer
            .as_mut()
            .ok_or_else(|| PocError::Window("missing egui renderer".to_owned()))?;
        let raw = state.take_egui_input(&self.window);
        let full = ctx.run_ui(raw, |ui| {
            egui::Area::new(egui::Id::new("bubble"))
                .fixed_pos(egui::pos2(40.0, 80.0))
                .show(ui.ctx(), |ui| {
                    egui::Frame::NONE
                        .fill(egui::Color32::from_rgba_unmultiplied(20, 32, 51, 153))
                        .corner_radius(18.0)
                        .inner_margin(12.0)
                        .show(ui, |ui| {
                            ui.label("egui overlay");
                            ui.label("C4 equivalent bubble");
                            if ui.button("Tap the bubble").clicked() {
                                tracing::trace!("egui equivalent bubble clicked");
                            }
                        });
                });
        });
        state.handle_platform_output(&self.window, full.platform_output);
        #[expect(
            clippy::cast_possible_truncation,
            reason = "window scale factors are small"
        )]
        let pixels_per_point = self.window.scale_factor() as f32;
        let screen = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [self.config.width.max(1), self.config.height.max(1)],
            pixels_per_point,
        };
        let primitives = ctx.tessellate(full.shapes, pixels_per_point);
        for (id, delta) in &full.textures_delta.set {
            renderer.update_texture(&self.gpu.device, &self.gpu.queue, *id, delta);
        }
        let mut encoder = self
            .gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("ene-stage-poc.c.egui"),
            });
        let extra = renderer.update_buffers(
            &self.gpu.device,
            &self.gpu.queue,
            &mut encoder,
            &primitives,
            &screen,
        );
        {
            let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("ene-stage-poc.c.egui"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
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
            renderer.render(&mut pass.forget_lifetime(), &primitives, &screen);
        }
        for id in &full.textures_delta.free {
            renderer.free_texture(id);
        }
        self.gpu
            .queue
            .submit(extra.into_iter().chain(std::iter::once(encoder.finish())));
        Ok(())
    }

    fn finish(&mut self) {
        let info = self.gpu.adapter.get_info();
        let reports = self.metrics.reports();
        let extra = format!(
            "case={case} profile={profile} vrm={vrm} composite={composite} ui_path={ui_path} blit=fullscreen-premul-alpha-pass copy_texture_to_texture=false cpu_readback=false gpu_resident=true gpu_render_pass_composite={composite} startup_ms={startup:.1} driver={driver} driver_info={driver_info}",
            case = self.case.name(),
            profile = if cfg!(debug_assertions) {
                "debug"
            } else {
                "release"
            },
            vrm = self.vrm.source(),
            composite = self.case.composite(),
            ui_path = match self.case {
                Case::C0 => "vrm-only",
                Case::C1 | Case::C2 | Case::C3 => "slint-offscreen-blit",
                Case::C4 => "egui-swapchain-load-pass",
            },
            startup = self.startup.unwrap_or(Duration::ZERO).as_secs_f64() * 1000.0,
            driver = info.driver,
            driver_info = info.driver_info,
        );
        metrics::print_reports(
            self.case.name(),
            &info.name,
            &format!("{:?}", info.backend),
            &extra,
            &reports,
        );
    }
}
