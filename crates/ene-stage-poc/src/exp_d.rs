//! Experiment D: Linux OS input region / click-through.

use std::sync::Arc;
use std::time::{Duration, Instant};

use winit::application::ApplicationHandler;
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowId, WindowLevel};

use crate::PocError;
use crate::blit::UiBlit;
use crate::gpu::{self, GpuContext};
use crate::input::ScreenPoint;
use crate::os_input::{NativeInputRegion, StageInputRegion};
use crate::region::{
    build_input_regions, classify_pointer, regions_dirty, should_apply_region, vrm_regions,
};
use crate::slint_host::{self, SlintHost};
use crate::vrm_scene::VrmScene;

pub fn run() -> Result<(), PocError> {
    crate::init_tracing();
    let event_loop = EventLoop::new().map_err(|err| PocError::Window(err.to_string()))?;
    let mut app = DApp {
        inner: None,
        error: None,
    };
    event_loop
        .run_app(&mut app)
        .map_err(|err| PocError::Window(err.to_string()))?;
    app.error.take().map_or(Ok(()), Err)
}

struct DApp {
    inner: Option<DRun>,
    error: Option<PocError>,
}

struct DRun {
    gpu: GpuContext,
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    depth: wgpu::Texture,
    depth_view: wgpu::TextureView,
    ui_tex: wgpu::Texture,
    ui_view: wgpu::TextureView,
    blit: UiBlit,
    vrm: VrmScene,
    host: SlintHost,
    input: NativeInputRegion,
    last_rects: Vec<crate::input::ScreenRect>,
    last_apply: Option<Instant>,
    cursor: Option<slint::LogicalPosition>,
    started: Instant,
    moving: bool,
    bench_until: Option<Instant>,
    gen_ns: u128,
    apply_ns: u128,
    apply_count: u32,
    gen_count: u32,
    threshold: f32,
    min_interval: Duration,
    dump_bits: u8,
}

impl ApplicationHandler for DApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.inner.is_some() {
            return;
        }
        match DRun::create(event_loop) {
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
        match &event {
            WindowEvent::Resized(size) => inner.resize(*size),
            WindowEvent::RedrawRequested => {
                if let Err(err) = inner.render() {
                    tracing::warn!(error = %err, "experiment D frame failed");
                }
            }
            WindowEvent::CloseRequested => {
                inner.finish();
                event_loop.exit();
            }
            WindowEvent::KeyboardInput { event: key, .. }
                if key.state == ElementState::Pressed
                    && matches!(key.logical_key, Key::Named(NamedKey::Escape)) =>
            {
                inner.finish();
                event_loop.exit();
            }
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } => inner.log_click(),
            _ => {}
        }
        let scale = inner.window.scale_factor();
        slint_host::dispatch_winit_event(&inner.host.ui, &event, scale, &mut inner.cursor);
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let Some(inner) = self.inner.as_mut() else {
            return;
        };
        if let Some(deadline) = inner.bench_until
            && Instant::now() >= deadline
        {
            inner.finish();
            event_loop.exit();
            return;
        }
        inner.sync_region();
        inner.maybe_dump_shape();
        if inner.moving || inner.started.elapsed() < Duration::from_millis(400) {
            inner.window.request_redraw();
            event_loop.set_control_flow(if inner.moving {
                ControlFlow::Poll
            } else {
                ControlFlow::WaitUntil(Instant::now() + Duration::from_millis(50))
            });
        } else {
            event_loop.set_control_flow(ControlFlow::WaitUntil(
                inner
                    .bench_until
                    .unwrap_or_else(|| Instant::now() + Duration::from_millis(250)),
            ));
        }
    }
}

impl DRun {
    fn create(event_loop: &ActiveEventLoop) -> Result<Self, PocError> {
        let gpu = pollster::block_on(GpuContext::create())?;
        let info = gpu.adapter.get_info();
        let native_wayland =
            std::env::var_os("WAYLAND_DISPLAY").is_some() && std::env::var_os("DISPLAY").is_none();
        tracing::info!(
            adapter = %info.name,
            backend = ?info.backend,
            wayland_display = ?std::env::var("WAYLAND_DISPLAY").ok(),
            display = ?std::env::var("DISPLAY").ok(),
            native_wayland,
            "experiment D gpu / display"
        );
        let mut attrs = Window::default_attributes()
            .with_title("ene-stage-poc-d")
            .with_inner_size(PhysicalSize::new(800, 600))
            .with_position(PhysicalPosition::new(80, 80))
            .with_transparent(true)
            .with_decorations(env_flag("ENE_STAGE_POC_DECORATED"))
            .with_window_level(WindowLevel::AlwaysOnTop);
        #[cfg(target_os = "linux")]
        {
            use winit::platform::x11::WindowAttributesExtX11;
            if env_flag("ENE_STAGE_POC_OVERRIDE_REDIRECT") {
                attrs = attrs.with_override_redirect(true);
                tracing::info!("override_redirect=true");
            }
        }
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
        let (ui_tex, ui_view) = gpu::create_ui_target(&gpu.device, config.width, config.height);
        let blit = UiBlit::new(&gpu.device, format);
        let vrm = VrmScene::load(&gpu.device, &gpu.queue, format).map_err(PocError::Window)?;
        let host = slint_host::install(Arc::clone(&window), gpu.handles())?;
        host.ui.set_show_bubble(true);
        host.ui.set_show_menu(env_flag("ENE_STAGE_POC_SHOW_MENU"));
        let input = NativeInputRegion::attach(Arc::clone(&window));
        let bench = std::env::var("ENE_STAGE_POC_SECONDS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .filter(|s| *s > 0)
            .map(Duration::from_secs);
        let started = Instant::now();
        window.request_redraw();
        Ok(Self {
            gpu,
            window,
            surface,
            config,
            depth,
            depth_view,
            ui_tex,
            ui_view,
            blit,
            vrm,
            host,
            input,
            last_rects: Vec::new(),
            last_apply: None,
            cursor: None,
            started,
            moving: env_flag("ENE_STAGE_POC_MOVE_VRM") || env_flag("ENE_STAGE_POC_ANIMATE"),
            bench_until: bench.map(|d| started + d),
            gen_ns: 0,
            apply_ns: 0,
            apply_count: 0,
            gen_count: 0,
            threshold: env_f32("ENE_STAGE_POC_REGION_PX", 2.0),
            min_interval: Duration::from_millis(
                std::env::var("ENE_STAGE_POC_REGION_MS")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(16),
            ),
            dump_bits: 0,
        })
    }

    fn resize(&mut self, size: PhysicalSize<u32>) {
        if size.width == 0 || size.height == 0 {
            return;
        }
        self.config.width = size.width;
        self.config.height = size.height;
        self.surface.configure(&self.gpu.device, &self.config);
        let (depth, view) = gpu::create_depth(&self.gpu.device, size.width, size.height);
        self.depth = depth;
        self.depth_view = view;
        let (tex, ui_view) = gpu::create_ui_target(&self.gpu.device, size.width, size.height);
        self.ui_tex = tex;
        self.ui_view = ui_view;
        self.host.adapter.set_size(size.width, size.height);
        self.window.request_redraw();
    }

    fn render(&mut self) -> Result<(), PocError> {
        if self.moving {
            let t = self.started.elapsed().as_secs_f32();
            self.host.ui.set_bubble_y(80.0 + t.sin() * 30.0);
        }
        let frame = gpu::acquire_frame(&self.surface).map_err(PocError::Surface)?;
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("ene-stage-poc.d.vrm"),
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
        slint::platform::update_timers_and_animations();
        self.host
            .adapter
            .render_ui(&self.ui_view, self.config.width, self.config.height)?;
        let mut encoder = self
            .gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("ene-stage-poc.d.ui"),
            });
        self.blit
            .draw(&self.gpu.device, &mut encoder, &self.ui_view, &view);
        self.gpu.queue.submit(std::iter::once(encoder.finish()));
        frame.present();
        self.sync_region();
        Ok(())
    }

    fn sync_region(&mut self) {
        let scale = slint_host::scale_f32(self.window.scale_factor());
        let t0 = Instant::now();
        let ui = slint_host::ui_regions(&self.host.ui, scale);
        let mut layout = self.vrm.hit_layout((self.config.width, self.config.height));
        if self.moving {
            let wave = self.started.elapsed().as_secs_f32().sin() * 24.0;
            layout.torso.x += wave;
            layout.head.x += wave;
            layout.left_hand.x += wave;
            layout.right_hand.x += wave;
        }
        let next = build_input_regions(&ui, &vrm_regions(Some(&layout)));
        self.gen_ns += t0.elapsed().as_nanos();
        self.gen_count = self.gen_count.saturating_add(1);
        let dirty = regions_dirty(&self.last_rects, &next, self.threshold);
        let now = Instant::now();
        if should_apply_region(dirty, self.last_apply, self.min_interval, now) {
            let t1 = Instant::now();
            self.input.update_input_region(&next);
            self.apply_ns += t1.elapsed().as_nanos();
            self.apply_count = self.apply_count.saturating_add(1);
            self.last_apply = Some(now);
            self.last_rects = next;
        }
    }

    fn maybe_dump_shape(&mut self) {
        let ms = self.started.elapsed().as_millis();
        let (bit, tag) = if ms >= 1000 && self.dump_bits & 4 == 0 {
            (4_u8, "t=1000ms")
        } else if ms >= 200 && self.dump_bits & 2 == 0 {
            (2, "t=200ms")
        } else if ms >= 50 && self.dump_bits & 1 == 0 {
            (1, "t=50ms")
        } else {
            return;
        };
        self.dump_bits |= bit;
        self.input.debug_dump(tag);
    }

    fn log_click(&self) {
        let scale = slint_host::scale_f32(self.window.scale_factor());
        let Some(pos) = self.cursor else {
            return;
        };
        let cursor = ScreenPoint {
            x: pos.x * scale,
            y: pos.y * scale,
        };
        let ui = slint_host::ui_regions(&self.host.ui, scale);
        let layout = self.vrm.hit_layout((self.config.width, self.config.height));
        let route = classify_pointer(cursor, &ui, Some(&layout));
        println!(
            "UI hit: {}  VRM hit: {:?}  OS region hit: {}  target: {:?}",
            route.ui_hit, route.vrm_hit, route.os_region_hit, route.target
        );
        tracing::info!(
            ui_hit = route.ui_hit,
            vrm_hit = ?route.vrm_hit,
            os_region_hit = route.os_region_hit,
            ?route.target,
            "pointer route"
        );
    }

    fn finish(&mut self) {
        let info = self.gpu.adapter.get_info();
        let wall = self.started.elapsed().as_secs_f64().max(0.001);
        let apply_hz = f64::from(self.apply_count) / wall;
        println!("=== experiment-d ===");
        println!("adapter: {}", info.name);
        println!("backend: {:?}", info.backend);
        println!(
            "server={} native_wayland={} updates={} apply_hz={apply_hz:.2} last_os_us={} gen_avg_us={} apply_avg_us={}",
            self.input.display_server().name(),
            std::env::var_os("WAYLAND_DISPLAY").is_some() && std::env::var_os("DISPLAY").is_none(),
            self.input.os_update_count(),
            self.input.last_os_update().as_micros(),
            if self.gen_count == 0 {
                0
            } else {
                self.gen_ns / u128::from(self.gen_count) / 1000
            },
            if self.apply_count == 0 {
                0
            } else {
                self.apply_ns / u128::from(self.apply_count) / 1000
            },
        );
    }
}

fn env_flag(name: &str) -> bool {
    matches!(
        std::env::var(name).as_deref(),
        Ok("1" | "true" | "TRUE" | "yes")
    )
}

fn env_f32(name: &str, default: f32) -> f32 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}
