//! winit event loop driver.
//!
//! Owns the winit window, its wgpu surface, and the red-rect
//! renderer. Implements winit 0.30's `ApplicationHandler` so
//! `EventLoop::run_app` is a single call from `main`.
//!
//! PR0 scope (see `docs/architecture/wgpu-migration.md` §22.3): a
//! single transparent window, a red rect on top of a clear color,
//! `Space` toggles transparency, `Escape` / close exits. Tray, AI
//! bridge, egui, VRM, etc. arrive in later PRs.
use std::sync::Arc;
use wgpu::util::DeviceExt;
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalSize};
use winit::event::{ElementState, KeyEvent, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowAttributes, WindowId, WindowLevel};

use crate::gpu::{GpuContext, pick_format_and_alpha};

/// Top-level runtime. One per process.
pub struct Runtime {
    gpu: GpuContext,
    /// `true` = transparent window, no decorations, clear to
    /// `(0,0,0,0)`. `false` = opaque gray window with title bar.
    transparent: bool,
    slot: Option<WindowSlot>,
    rect: Option<RectRenderer>,
}

impl Runtime {
    pub fn new(gpu: GpuContext) -> Self {
        Self {
            gpu,
            transparent: false,
            slot: None,
            rect: None,
        }
    }
}

impl ApplicationHandler for Runtime {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.slot.is_some() {
            return;
        }
        let attrs = window_attributes(self.transparent);
        let window = match event_loop.create_window(attrs) {
            Ok(w) => Arc::new(w),
            Err(e) => {
                tracing::error!("Failed to create window: {e}");
                event_loop.exit();
                return;
            }
        };

        let size = window.inner_size();
        match WindowSlot::new(
            window,
            &self.gpu.instance,
            &self.gpu.adapter,
            &self.gpu.device,
            size,
        ) {
            Ok(slot) => {
                let format = slot.config.format;
                self.rect = Some(RectRenderer::new(&self.gpu.device, format));
                slot.window.request_redraw();
                self.slot = Some(slot);
            }
            Err(e) => {
                tracing::error!("Failed to create WindowSlot: {e}");
                event_loop.exit();
            }
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        let Some(slot) = self.slot.as_mut() else {
            return;
        };
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. } => {
                slot.reconfigure(&self.gpu.device, slot.window.inner_size());
                slot.window.request_redraw();
            }
            WindowEvent::KeyboardInput { .. } => match key_pressed(&event) {
                Some(NamedKey::Space) => {
                    self.transparent = !self.transparent;
                    slot.window.set_decorations(!self.transparent);
                    slot.window.request_redraw();
                }
                Some(NamedKey::Escape) => event_loop.exit(),
                _ => {}
            },
            WindowEvent::RedrawRequested => {
                let Some(rect) = self.rect.as_ref() else {
                    return;
                };
                if let Err(e) =
                    slot.render_frame(&self.gpu.device, &self.gpu.queue, rect, self.transparent)
                {
                    match e {
                        AcquireError::Reconfigure => {
                            tracing::warn!("Surface acquire Outdated/Lost; reconfiguring");
                            slot.reconfigure(&self.gpu.device, slot.window.inner_size());
                            slot.window.request_redraw();
                        }
                        AcquireError::Timeout => slot.window.request_redraw(),
                        AcquireError::Fatal => {
                            tracing::error!("Surface acquire failed fatally; exiting");
                            event_loop.exit();
                        }
                    }
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        if let Some(slot) = self.slot.as_ref() {
            slot.window.request_redraw();
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

/// Build the [`WindowAttributes`] used by the character window.
///
/// Mirrors the working-transparency recipe in `apps/tw-test` (which
/// runs the patched `bevy_winit` and sets both styles together at
/// `patches/bevy_winit/src/winit_windows.rs:133-146`):
/// * `transparent = transparent` — runtime knob. Drives the winit
///   `WindowFlags::TRANSPARENT` flag and `decorations`.
/// * `with_no_redirection_bitmap(true)` on Windows — **always** set,
///   regardless of `transparent`. Adds `WS_EX_NOREDIRECTIONBITMAP` to
///   the HWND exstyle at create time, which is what makes wgpu's
///   DX12 backend advertise `PreMultiplied` for the surface.
/// * `decorations = !transparent` — visible title bar in the opaque
///   state, none in the transparent state.
/// * `WindowLevel::AlwaysOnTop` — the character floats above other
///   windows. Cannot be changed at runtime in winit 0.30.
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

/// One window backed by one `wgpu::Surface`. The character window is
/// the only slot in the minimum v2 scaffold.
struct WindowSlot {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
}

impl WindowSlot {
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
        tracing::debug!(
            "wgpu surface capabilities: formats={:?}, alpha_modes={:?}",
            caps.formats,
            caps.alpha_modes
        );
        let (format, alpha_mode) = pick_format_and_alpha(&caps);
        if !caps.alpha_modes.contains(&alpha_mode) {
            tracing::warn!(
                "alpha_mode={alpha_mode:?} not in surface caps ({:?}); surface.configure() may fail",
                caps.alpha_modes
            );
        }
        tracing::debug!(
            "SurfaceConfiguration picked: format={format:?}, alpha_mode={alpha_mode:?}"
        );

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
        let frame = self.surface.get_current_texture().map_err(|e| match e {
            wgpu::SurfaceError::Outdated | wgpu::SurfaceError::Lost => AcquireError::Reconfigure,
            wgpu::SurfaceError::Timeout => AcquireError::Timeout,
            wgpu::SurfaceError::OutOfMemory | wgpu::SurfaceError::Other => AcquireError::Fatal,
        })?;
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
            });
            rect.draw(&mut rp);
        }
        queue.submit(std::iter::once(encoder.finish()));
        frame.present();
        Ok(())
    }
}

/// Errors from acquiring the current swap-chain image.
enum AcquireError {
    /// The surface configuration is stale; caller should reconfigure
    /// and request a redraw.
    Reconfigure,
    /// Surface acquire timed out; safe to ignore and retry.
    Timeout,
    /// Out of memory or backend-reported unrecoverable error.
    Fatal,
}

/// Single colored rectangle renderer (Bevy-logo replacement).
///
/// The PR0 quad sits at NDC `[-0.5, 0.5]²` with a hardcoded
/// identity transform, hardcoded color, and a six-vertex
/// `TriangleList` (two triangles, no index buffer). That keeps the
/// pipeline layout empty (no bind group) and the shader to a handful
/// of WGSL lines.
struct RectRenderer {
    pipeline: wgpu::RenderPipeline,
    vbuf: wgpu::Buffer,
}

const QUAD: &[glam::Vec2] = &[
    // Triangle 1: BL, BR, TR
    glam::Vec2::new(-0.5, -0.5),
    glam::Vec2::new(0.5, -0.5),
    glam::Vec2::new(0.5, 0.5),
    // Triangle 2: BL, TR, TL
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
            push_constant_ranges: &[],
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
            multiview: None,
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
