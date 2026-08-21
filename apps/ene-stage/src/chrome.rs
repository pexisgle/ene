//! Independent egui windows hosted on winit + wgpu.

use std::sync::Arc;

use egui::ViewportId;
use egui_wgpu::{Renderer, RendererOptions, ScreenDescriptor};
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::{Window, WindowId};

use crate::gpu::{self, GpuContext, GpuError};
use crate::i18n;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChromeKind {
    Chat,
    Detail,
    Caption,
    Spotlight,
}

impl ChromeKind {
    #[must_use]
    pub fn title(self) -> String {
        match self {
            Self::Chat => i18n::fl("surface-title"),
            Self::Detail => i18n::fl("detail-title"),
            Self::Caption => i18n::fl("caption-title"),
            Self::Spotlight => i18n::fl("spotlight-title"),
        }
    }
}

/// One native window with its own egui context.
pub struct ChromeWindow {
    pub kind: ChromeKind,
    pub window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    #[expect(dead_code, reason = "kept for surface recreation")]
    format: wgpu::TextureFormat,
    egui_ctx: egui::Context,
    egui_state: egui_winit::State,
    renderer: Renderer,
}

impl ChromeWindow {
    pub fn raise(&self) {
        self.window
            .set_window_level(winit::window::WindowLevel::AlwaysOnTop);
        self.window.focus_window();
    }

    pub fn create(
        event_loop: &ActiveEventLoop,
        gpu: &GpuContext,
        kind: ChromeKind,
        inner: PhysicalSize<u32>,
        decorations: bool,
    ) -> Result<Self, GpuError> {
        let attrs = Window::default_attributes()
            .with_title(kind.title())
            .with_inner_size(inner)
            .with_decorations(decorations)
            .with_transparent(kind == ChromeKind::Caption || kind == ChromeKind::Spotlight)
            .with_window_level(winit::window::WindowLevel::AlwaysOnTop);
        let window = Arc::new(
            event_loop
                .create_window(attrs)
                .map_err(|err| GpuError::Surface(err.to_string()))?,
        );
        if let Some(monitor) = window.current_monitor() {
            match kind {
                ChromeKind::Caption => {
                    place_caption_window(&window, "bottom");
                }
                ChromeKind::Chat => {
                    let origin = monitor.position();
                    window.set_outer_position(PhysicalPosition::new(origin.x + 24, origin.y + 80));
                }
                ChromeKind::Detail | ChromeKind::Spotlight => {}
            }
        }
        let surface = gpu.create_surface(Arc::clone(&window))?;
        let format = gpu.surface_format(&surface);
        let alpha = gpu::pick_alpha_mode(&surface, &gpu.adapter);
        let config =
            gpu::configure_surface(&surface, &gpu.device, format, window.inner_size(), alpha);
        let egui_ctx = egui::Context::default();
        let egui_state = egui_winit::State::new(
            egui_ctx.clone(),
            ViewportId::ROOT,
            window.as_ref(),
            Some(window.scale_factor() as f32),
            None,
            None,
        );
        let renderer = Renderer::new(
            &gpu.device,
            format,
            RendererOptions {
                msaa_samples: 1,
                depth_stencil_format: None,
                dithering: true,
                predictable_texture_filtering: false,
            },
        );
        Ok(Self {
            kind,
            window,
            surface,
            config,
            format,
            egui_ctx,
            egui_state,
            renderer,
        })
    }

    #[must_use]
    pub fn id(&self) -> WindowId {
        self.window.id()
    }

    #[must_use]
    pub fn wants_keyboard_input(&self) -> bool {
        self.egui_ctx.egui_wants_keyboard_input()
    }

    pub fn place_caption(&self, position: &str) {
        if self.kind != ChromeKind::Caption {
            return;
        }
        place_caption_window(&self.window, position);
    }

    pub fn on_window_event(&mut self, event: &WindowEvent) -> bool {
        self.egui_state.on_window_event(&self.window, event).repaint
    }

    pub fn resize(&mut self, gpu: &GpuContext, size: PhysicalSize<u32>) {
        if size.width == 0 || size.height == 0 {
            return;
        }
        self.config.width = size.width;
        self.config.height = size.height;
        self.surface.configure(&gpu.device, &self.config);
    }

    pub fn paint(
        &mut self,
        gpu: &GpuContext,
        mut add_contents: impl FnMut(&mut egui::Ui),
    ) -> Result<(), GpuError> {
        let raw = self.egui_state.take_egui_input(&self.window);
        let full = self.egui_ctx.run_ui(raw, |ui| add_contents(ui));
        self.egui_state
            .handle_platform_output(&self.window, full.platform_output);

        let size = self.window.inner_size();
        let pixels_per_point = self.window.scale_factor() as f32;
        let screen = ScreenDescriptor {
            size_in_pixels: [size.width.max(1), size.height.max(1)],
            pixels_per_point,
        };
        let primitives = self.egui_ctx.tessellate(full.shapes, pixels_per_point);
        for (id, delta) in &full.textures_delta.set {
            self.renderer
                .update_texture(&gpu.device, &gpu.queue, *id, delta);
        }
        let mut encoder = gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("ene-stage.chrome"),
            });
        let extra = self.renderer.update_buffers(
            &gpu.device,
            &gpu.queue,
            &mut encoder,
            &primitives,
            &screen,
        );
        let frame = gpu::acquire_frame(&self.surface).map_err(GpuError::Surface)?;
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let clear = if self.kind == ChromeKind::Caption || self.kind == ChromeKind::Spotlight {
            wgpu::Color::TRANSPARENT
        } else {
            wgpu::Color {
                r: 0.07,
                g: 0.07,
                b: 0.09,
                a: 1.0,
            }
        };
        {
            let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("ene-stage.chrome.pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(clear),
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
        gpu.queue
            .submit(extra.into_iter().chain(std::iter::once(encoder.finish())));
        frame.present();
        Ok(())
    }
}

fn place_caption_window(window: &Window, position: &str) {
    let Some(monitor) = window.current_monitor() else {
        return;
    };
    let screen = monitor.size();
    let origin = monitor.position();
    let inner = window.inner_size();
    let (x, y) = crate::surface::caption::outer_offset(
        position,
        (screen.width, screen.height),
        (inner.width, inner.height),
    );
    window.set_outer_position(PhysicalPosition::new(
        origin.x + i32::try_from(x).unwrap_or(i32::MAX),
        origin.y + i32::try_from(y).unwrap_or(i32::MAX),
    ));
}
