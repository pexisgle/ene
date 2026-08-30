//! Experiment D2: X11 visual footprint vs interaction footprint.

use crate::PocError;

pub fn run() -> Result<(), PocError> {
    crate::init_tracing();
    #[cfg(target_os = "linux")]
    {
        linux::run_linux()
    }
    #[cfg(not(target_os = "linux"))]
    {
        println!("D2 is Linux/X11 only");
        Ok(())
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use winit::application::ApplicationHandler;
    use winit::dpi::{PhysicalPosition, PhysicalSize};
    use winit::event::{ElementState, MouseButton, WindowEvent};
    use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
    use winit::keyboard::{Key, NamedKey};
    use winit::window::{Window, WindowId, WindowLevel};

    use super::PocError;
    use crate::blit::UiBlit;
    use crate::gpu::{self, GpuContext};
    use crate::input::{ScreenPoint, ScreenRect, aabb_union};
    use crate::metrics::Snapshot;
    use crate::os_input::DisplayServer;
    use crate::region::{
        build_interaction_region, build_visual_region, classify_layers, regions_dirty,
        should_apply_region, vrm_regions, vrm_visual_regions,
    };
    use crate::slint_host::{self, SlintHost};
    use crate::vrm_scene::VrmScene;
    use crate::x11_split::{X11SplitShapes, fmt_rects};

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Case {
        InputOnly,
        BothEqual,
        T1,
        T3,
        T4,
        T5,
        T5m,
        T6,
        T7u,
        T7d,
        T7o,
    }

    impl Case {
        #[must_use]
        pub fn parse(raw: &str) -> Option<Self> {
            match raw.trim().to_ascii_uppercase().as_str() {
                "INPUT" | "T0" | "INPUTONLY" => Some(Self::InputOnly),
                "BOTH" | "BOTHEQUAL" => Some(Self::BothEqual),
                "T1" => Some(Self::T1),
                "T3" => Some(Self::T3),
                "T4" => Some(Self::T4),
                "T5" => Some(Self::T5),
                "T5M" => Some(Self::T5m),
                "T6" => Some(Self::T6),
                "T7U" => Some(Self::T7u),
                "T7D" => Some(Self::T7d),
                "T7O" => Some(Self::T7o),
                _ => None,
            }
        }

        #[must_use]
        pub const fn cli(self) -> &'static str {
            match self {
                Self::InputOnly => "input",
                Self::BothEqual => "both",
                Self::T1 => "T1",
                Self::T3 => "T3",
                Self::T4 => "T4",
                Self::T5 => "T5",
                Self::T5m => "T5m",
                Self::T6 => "T6",
                Self::T7u => "T7u",
                Self::T7d => "T7d",
                Self::T7o => "T7o",
            }
        }

        #[must_use]
        pub const fn name(self) -> &'static str {
            match self {
                Self::InputOnly => "input-only",
                Self::BothEqual => "bounding-plus-input",
                Self::T1 => "t1-bounding-gt-input",
                Self::T3 => "t3-visual-only-slint",
                Self::T4 => "t4-bounding-clip",
                Self::T5 => "t5-visual-vs-input",
                Self::T5m => "t5-moving",
                Self::T6 => "t6-shapenotify-reapply",
                Self::T7u => "t7-undecorated",
                Self::T7d => "t7-decorated",
                Self::T7o => "t7-override-redirect",
            }
        }

        const fn draw_vrm(self) -> bool {
            !matches!(self, Self::T1)
        }

        const fn show_field(self) -> bool {
            matches!(self, Self::T1)
        }

        const fn show_glow(self) -> bool {
            matches!(
                self,
                Self::T3
                    | Self::T4
                    | Self::T5
                    | Self::T5m
                    | Self::T6
                    | Self::T7u
                    | Self::T7d
                    | Self::T7o
            )
        }

        const fn decorations(self) -> bool {
            matches!(self, Self::T7d)
        }

        const fn override_redirect(self) -> bool {
            matches!(self, Self::T7o)
        }

        const fn reapply(self) -> bool {
            matches!(self, Self::T6)
        }

        const fn moving(self) -> bool {
            matches!(self, Self::T5m)
        }

        const fn bounding_from_visual(self) -> bool {
            !matches!(self, Self::T4 | Self::InputOnly | Self::BothEqual)
        }

        const fn input_only(self) -> bool {
            matches!(self, Self::InputOnly)
        }

        const fn default_secs(self) -> u64 {
            match self {
                Self::T6 => 30,
                Self::T5m => 12,
                _ => 8,
            }
        }
    }

    const ALL: [Case; 11] = [
        Case::InputOnly,
        Case::BothEqual,
        Case::T1,
        Case::T3,
        Case::T4,
        Case::T5,
        Case::T5m,
        Case::T6,
        Case::T7u,
        Case::T7d,
        Case::T7o,
    ];

    pub fn run_linux() -> Result<(), PocError> {
        let arg = std::env::args()
            .nth(1)
            .or_else(|| std::env::var("ENE_STAGE_POC_CASE").ok());
        if arg.as_deref().is_none_or(|v| v == "all") {
            return run_all();
        }
        let Some(case) = arg.as_deref().and_then(Case::parse) else {
            return Err(PocError::Window(
                "usage: ene-stage-poc-x11-shape T1|T3|T4|T5|T5m|T6|T7u|T7d|T7o|input|both|all"
                    .to_owned(),
            ));
        };
        run_case(case)
    }

    #[cfg(target_os = "linux")]
    fn run_all() -> Result<(), PocError> {
        let exe = std::env::current_exe().map_err(|err| PocError::Window(err.to_string()))?;
        for case in ALL {
            tracing::info!(case = case.name(), "spawn experiment D2 case");
            let status = std::process::Command::new(&exe)
                .arg(case.cli())
                .env("ENE_STAGE_POC_CASE", case.cli())
                .status()
                .map_err(|err| PocError::Window(err.to_string()))?;
            if !status.success() {
                return Err(PocError::Window(format!("{} exited {status}", case.name())));
            }
        }
        Ok(())
    }

    #[cfg(target_os = "linux")]
    fn run_case(case: Case) -> Result<(), PocError> {
        let event_loop = EventLoop::new().map_err(|err| PocError::Window(err.to_string()))?;
        let mut app = D2App {
            case,
            inner: None,
            error: None,
        };
        event_loop
            .run_app(&mut app)
            .map_err(|err| PocError::Window(err.to_string()))?;
        app.error.take().map_or(Ok(()), Err)
    }

    #[cfg(target_os = "linux")]
    struct D2App {
        case: Case,
        inner: Option<D2Run>,
        error: Option<PocError>,
    }

    #[cfg(target_os = "linux")]
    struct D2Run {
        case: Case,
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
        shapes: Option<X11SplitShapes>,
        last_visual: Vec<ScreenRect>,
        last_interaction: Vec<ScreenRect>,
        last_bounding_apply: Option<Instant>,
        last_input_apply: Option<Instant>,
        cursor: Option<slint::LogicalPosition>,
        started: Instant,
        bench_until: Instant,
        threshold: f32,
        min_interval: Duration,
        dump_bits: u8,
        cpu0: Snapshot,
        overlay_presses: u32,
    }

    #[cfg(target_os = "linux")]
    impl ApplicationHandler for D2App {
        fn resumed(&mut self, event_loop: &ActiveEventLoop) {
            if self.inner.is_some() {
                return;
            }
            match D2Run::create(event_loop, self.case) {
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
                        tracing::warn!(error = %err, "experiment D2 frame failed");
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
                    state,
                    button: MouseButton::Left,
                    ..
                } => inner.log_click(*state),
                _ => {}
            }
            let scale = inner.window.scale_factor();
            slint_host::dispatch_winit_event(&inner.host.ui, &event, scale, &mut inner.cursor);
        }

        fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
            let Some(inner) = self.inner.as_mut() else {
                return;
            };
            if Instant::now() >= inner.bench_until {
                inner.finish();
                event_loop.exit();
                return;
            }
            inner.tick_shapes();
            inner.maybe_dump_shape();
            inner.window.request_redraw();
            event_loop.set_control_flow(if inner.case.moving() {
                ControlFlow::Poll
            } else {
                ControlFlow::WaitUntil(Instant::now() + Duration::from_millis(50))
            });
        }
    }

    #[cfg(target_os = "linux")]
    impl D2Run {
        fn create(event_loop: &ActiveEventLoop, case: Case) -> Result<Self, PocError> {
            let gpu = pollster::block_on(GpuContext::create())?;
            let info = gpu.adapter.get_info();
            tracing::info!(
                case = case.name(),
                adapter = %info.name,
                backend = ?info.backend,
                display = ?std::env::var("DISPLAY").ok(),
                "experiment D2 gpu / display"
            );
            let mut attrs = Window::default_attributes()
                .with_title(format!("ene-stage-poc-x11-shape {}", case.name()))
                .with_inner_size(PhysicalSize::new(800, 600))
                .with_position(PhysicalPosition::new(80, 80))
                .with_transparent(true)
                .with_decorations(case.decorations())
                .with_window_level(WindowLevel::AlwaysOnTop);
            {
                use winit::platform::x11::WindowAttributesExtX11;
                if case.override_redirect() {
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
            configure_ui(&host, case);
            let shapes = X11SplitShapes::try_new(window.as_ref());
            if shapes.is_none() {
                tracing::warn!("X11 split backend missing (not an X11 window?)");
            }
            let secs = std::env::var("ENE_STAGE_POC_SECONDS")
                .ok()
                .and_then(|v| v.parse::<u64>().ok())
                .filter(|s| *s > 0)
                .unwrap_or(case.default_secs());
            let started = Instant::now();
            println!(
                "D2 case={} server={} draw_vrm={} reapply={} moving={}",
                case.name(),
                DisplayServer::detect(window.as_ref()).name(),
                case.draw_vrm(),
                case.reapply(),
                case.moving()
            );
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
                ui_view,
                blit,
                vrm,
                host,
                shapes,
                last_visual: Vec::new(),
                last_interaction: Vec::new(),
                last_bounding_apply: None,
                last_input_apply: None,
                cursor: None,
                started,
                bench_until: started + Duration::from_secs(secs),
                threshold: env_f32("ENE_STAGE_POC_REGION_PX", 2.0),
                min_interval: Duration::from_millis(
                    std::env::var("ENE_STAGE_POC_REGION_MS")
                        .ok()
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(50),
                ),
                dump_bits: 0,
                cpu0: Snapshot::now(),
                overlay_presses: 0,
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
            if self.case.moving() {
                let t = self.started.elapsed().as_secs_f32();
                self.host.ui.set_bubble_y(80.0 + t.sin() * 28.0);
                self.host.ui.set_particle_x(60.0 + t.cos() * 40.0);
            }
            let frame = gpu::acquire_frame(&self.surface).map_err(PocError::Surface)?;
            let view = frame
                .texture
                .create_view(&wgpu::TextureViewDescriptor::default());
            let mut encoder =
                self.gpu
                    .device
                    .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                        label: Some("ene-stage-poc.d2.scene"),
                    });
            if self.case.draw_vrm() {
                self.vrm.render(
                    &self.gpu.queue,
                    &mut encoder,
                    &view,
                    &self.depth_view,
                    self.config.width,
                    self.config.height,
                    true,
                );
            } else {
                drop(encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("ene-stage-poc.d2.clear"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &view,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color {
                                r: 0.0,
                                g: 0.0,
                                b: 0.0,
                                a: 0.0,
                            }),
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
                }));
            }
            self.gpu.queue.submit(std::iter::once(encoder.finish()));
            slint::platform::update_timers_and_animations();
            self.host
                .adapter
                .render_ui(&self.ui_view, self.config.width, self.config.height)?;
            let mut encoder =
                self.gpu
                    .device
                    .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                        label: Some("ene-stage-poc.d2.ui"),
                    });
            self.blit
                .draw(&self.gpu.device, &mut encoder, &self.ui_view, &view);
            self.gpu.queue.submit(std::iter::once(encoder.finish()));
            frame.present();
            self.sync_regions();
            Ok(())
        }

        fn scene(&self) -> (Vec<ScreenRect>, Vec<ScreenRect>) {
            let scale = slint_host::scale_f32(self.window.scale_factor());
            let mut layout = self.vrm.hit_layout((self.config.width, self.config.height));
            if self.case.moving() {
                let wave = self.started.elapsed().as_secs_f32().sin() * 24.0;
                layout.torso.x += wave;
                layout.head.x += wave;
                layout.left_hand.x += wave;
                layout.right_hand.x += wave;
            }
            let mut visual_parts = slint_host::visual_decoration_rects(&self.host.ui, scale);
            visual_parts.extend(slint_host::ui_regions(&self.host.ui, scale));
            if self.case.draw_vrm() {
                visual_parts.extend(vrm_visual_regions(Some(&layout), 12.0));
            }
            let mut interaction_parts = slint_host::ui_regions(&self.host.ui, scale);
            if self.case.draw_vrm() {
                interaction_parts.extend(vrm_regions(Some(&layout)));
            }
            if self.case == Case::T1 {
                visual_parts = vec![ScreenRect::new(100.0, 100.0, 600.0, 400.0)];
                interaction_parts = vec![ScreenRect::new(300.0, 250.0, 200.0, 100.0)];
            }
            (
                build_visual_region(&visual_parts),
                build_interaction_region(&interaction_parts),
            )
        }

        fn sync_regions(&mut self) {
            let (visual, interaction) = self.scene();
            let bounding = if self.case.bounding_from_visual() {
                visual.clone()
            } else {
                interaction.clone()
            };
            let input = interaction.clone();
            let visual_dirty = regions_dirty(&self.last_visual, &visual, self.threshold);
            let interaction_dirty = regions_dirty(&self.last_interaction, &input, self.threshold);
            let now = Instant::now();
            let apply_b = should_apply_region(
                visual_dirty || self.last_bounding_apply.is_none(),
                self.last_bounding_apply,
                self.min_interval,
                now,
            );
            let apply_i = should_apply_region(
                interaction_dirty || self.last_input_apply.is_none(),
                self.last_input_apply,
                self.min_interval,
                now,
            );
            let Some(shapes) = self.shapes.as_mut() else {
                self.last_visual = visual;
                self.last_interaction = interaction;
                return;
            };
            if self.case.input_only() {
                if apply_i {
                    let dt = shapes.set_input(&input);
                    println!(
                        "SET Input only us={} rects={}",
                        dt.as_micros(),
                        fmt_rects(&input)
                    );
                    self.last_input_apply = Some(now);
                }
            } else if apply_b && apply_i {
                let dt = shapes.set_split(&bounding, &input);
                println!(
                    "SET combined us={} Bounding={} Input={}",
                    dt.as_micros(),
                    fmt_rects(&bounding),
                    fmt_rects(&input)
                );
                self.last_bounding_apply = Some(now);
                self.last_input_apply = Some(now);
            } else if apply_b {
                let dt = shapes.set_bounding(&bounding);
                println!(
                    "SET Bounding us={} rects={}",
                    dt.as_micros(),
                    fmt_rects(&bounding)
                );
                self.last_bounding_apply = Some(now);
            } else if apply_i {
                let dt = shapes.set_input(&input);
                println!(
                    "SET Input us={} rects={}",
                    dt.as_micros(),
                    fmt_rects(&input)
                );
                self.last_input_apply = Some(now);
            }
            self.last_visual = visual;
            self.last_interaction = interaction;
        }

        fn tick_shapes(&mut self) {
            let reapply = self.case.reapply();
            if let Some(shapes) = self.shapes.as_mut() {
                shapes.poll_notifies();
                if self.started.elapsed() >= Duration::from_millis(180) {
                    let _reset = shapes.detect_and_reapply_input(reapply);
                }
            }
            self.sync_regions();
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
            if let Some(shapes) = self.shapes.as_mut() {
                shapes.dump(tag);
            }
        }

        fn log_click(&mut self, state: ElementState) {
            let scale = slint_host::scale_f32(self.window.scale_factor());
            let Some(pos) = self.cursor else {
                return;
            };
            let cursor = ScreenPoint {
                x: pos.x * scale,
                y: pos.y * scale,
            };
            let (visual, interaction) = self.scene();
            let layer = classify_layers(cursor, &visual, &interaction);
            let kind = match state {
                ElementState::Pressed => "PRESS",
                ElementState::Released => "RELEASE",
            };
            if state == ElementState::Pressed {
                self.overlay_presses = self.overlay_presses.saturating_add(1);
            }
            println!(
                "OVERLAY {kind} x={:.0} y={:.0} layer={layer:?} visual={} interaction={}",
                cursor.x,
                cursor.y,
                fmt_rects(&visual),
                fmt_rects(&interaction)
            );
        }

        fn finish(&mut self) {
            let cpu1 = Snapshot::now();
            let wall = self.started.elapsed().as_secs_f64().max(0.001);
            if let Some(shapes) = self.shapes.as_mut() {
                shapes.dump("finish");
                let costs = shapes.costs;
                let bounding_us = avg_us(costs.bounding_ns, costs.bounding_sets);
                let input_us = avg_us(costs.input_ns, costs.input_sets);
                let combined_us = avg_us(costs.combined_ns, costs.combined_sets);
                let get_us = avg_us(costs.get_ns, costs.get_n);
                println!("=== experiment-d2 {} ===", self.case.name());
                println!(
                    "wall_s={wall:.1} overlay_presses={} bounding_sets={} input_sets={} combined_sets={} bounding_hz={:.2} input_hz={:.2}",
                    self.overlay_presses,
                    costs.bounding_sets,
                    costs.input_sets,
                    costs.combined_sets,
                    f64::from(costs.bounding_sets) / wall,
                    f64::from(costs.input_sets) / wall,
                );
                println!(
                    "SET_us bounding={bounding_us} input={input_us} combined={combined_us} get_rectangles={get_us}"
                );
                println!(
                    "ShapeNotify n={} input_notifies={} wm_resets={} reapplies={} reset_hz={:.2} reapply_hz={:.2}",
                    costs.notifies,
                    costs.input_notifies,
                    costs.wm_resets,
                    costs.reapplies,
                    f64::from(costs.wm_resets) / wall,
                    f64::from(costs.reapplies) / wall,
                );
                let fight = costs.reapplies > 8
                    && f64::from(costs.wm_resets) / wall > 1.0
                    && costs.reapplies + 2 >= costs.wm_resets;
                println!("wm_reapply_fight={fight}");
            } else {
                println!("=== experiment-d2 {} ===", self.case.name());
                println!("X11 split backend unavailable");
            }
            println!(
                "cpu_user_ms={} cpu_sys_ms={} rss_end_bytes={}",
                cpu1.cpu_user.saturating_sub(self.cpu0.cpu_user).as_millis(),
                cpu1.cpu_sys.saturating_sub(self.cpu0.cpu_sys).as_millis(),
                cpu1.rss_bytes
            );
            let (visual, interaction) = self.scene();
            println!(
                "final visual AABB={} interaction AABB={}",
                fmt_rects(std::slice::from_ref(&aabb_union(&visual))),
                fmt_rects(std::slice::from_ref(&aabb_union(&interaction)))
            );
        }
    }

    #[cfg(target_os = "linux")]
    fn configure_ui(host: &SlintHost, case: Case) {
        host.ui.set_show_bubble(true);
        host.ui.set_show_menu(false);
        host.ui.set_show_field(case.show_field());
        host.ui.set_show_glow(case.show_glow());
        host.ui.set_show_shadow(case.show_glow());
        host.ui.set_show_particle(case.show_glow());
        if case == Case::T1 {
            host.ui.set_bubble_x(300.0);
            host.ui.set_bubble_y(250.0);
            host.ui.set_bubble_w(200.0);
            host.ui.set_bubble_h(100.0);
            host.ui.set_status_text("CLICKABLE".into());
            host.ui.set_show_glow(false);
            host.ui.set_show_shadow(false);
            host.ui.set_show_particle(false);
        } else if case == Case::T4 {
            host.ui.set_particle_x(720.0);
            host.ui.set_particle_y(20.0);
            host.ui.set_status_text("T4 clip".into());
        } else {
            host.ui
                .set_status_text(format!("D2 {}", case.name()).into());
        }
    }

    #[cfg(target_os = "linux")]
    fn avg_us(total_ns: u128, n: u32) -> u128 {
        if n == 0 {
            0
        } else {
            total_ns / u128::from(n) / 1000
        }
    }

    #[cfg(target_os = "linux")]
    fn env_f32(name: &str, default: f32) -> f32 {
        std::env::var(name)
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(default)
    }
}
