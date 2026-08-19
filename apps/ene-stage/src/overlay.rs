//! Transparent always-on-top character overlay (wgpu, no egui).

use std::sync::Arc;
use std::time::Instant;

use glam::Vec3;
use winit::dpi::PhysicalSize;
use winit::window::{Window, WindowId};

use crate::avatar::CompanionAvatar;
use crate::gpu::{self, GpuContext, GpuError};

/// Native overlay window that draws VRM into a transparent swapchain.
pub struct OverlayWindow {
    pub window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    format: wgpu::TextureFormat,
    depth: wgpu::Texture,
    depth_view: wgpu::TextureView,
    pub avatar: Option<CompanionAvatar>,
    pub transparent: bool,
    pub click_through: bool,
    last_frame: Instant,
}

impl OverlayWindow {
    pub fn create(
        window: Arc<Window>,
        gpu: &GpuContext,
        transparent: bool,
    ) -> Result<Self, GpuError> {
        let surface = gpu.create_surface(Arc::clone(&window))?;
        let format = gpu.surface_format(&surface);
        let alpha = gpu::pick_alpha_mode(&surface, &gpu.adapter);
        let size = window.inner_size();
        let config = gpu::configure_surface(&surface, &gpu.device, format, size, alpha);
        let (depth, depth_view) = gpu::create_depth(&gpu.device, config.width, config.height);
        Ok(Self {
            window,
            surface,
            config,
            format,
            depth,
            depth_view,
            avatar: None,
            transparent,
            click_through: transparent,
            last_frame: Instant::now(),
        })
    }

    #[must_use]
    pub fn id(&self) -> WindowId {
        self.window.id()
    }

    #[must_use]
    pub fn format(&self) -> wgpu::TextureFormat {
        self.format
    }

    pub fn resize(&mut self, gpu: &GpuContext, size: PhysicalSize<u32>) {
        if size.width == 0 || size.height == 0 {
            return;
        }
        self.config.width = size.width;
        self.config.height = size.height;
        self.surface.configure(&gpu.device, &self.config);
        let (depth, depth_view) = gpu::create_depth(&gpu.device, size.width, size.height);
        self.depth = depth;
        self.depth_view = depth_view;
    }

    pub fn set_click_through(&mut self, enabled: bool) {
        self.click_through = enabled;
        if let Err(err) = self.window.set_cursor_hittest(!enabled) {
            tracing::debug!(error = %err, "cursor hittest unsupported");
        }
    }

    pub fn toggle_chrome(&mut self) {
        self.transparent = !self.transparent;
        self.window.set_decorations(!self.transparent);
        self.set_click_through(self.transparent);
    }

    pub fn load_avatar(
        &mut self,
        gpu: &GpuContext,
        path: &std::path::Path,
        motions_dir: Option<&std::path::Path>,
    ) -> Result<(), crate::avatar::AvatarError> {
        let mut avatar = CompanionAvatar::load(path, &gpu.device, &gpu.queue, self.format)?;
        if let Some(dir) = motions_dir {
            avatar.load_motions(dir);
        }
        self.avatar = Some(avatar);
        Ok(())
    }

    pub fn tick_and_render(
        &mut self,
        gpu: &GpuContext,
        look_at: Option<Vec3>,
        visemes: Option<ene_vrm::VisemeWeights>,
    ) -> Result<(), OverlayError> {
        let now = Instant::now();
        let dt = now.saturating_duration_since(self.last_frame).as_secs_f32();
        self.last_frame = now;
        if let Some(avatar) = self.avatar.as_mut() {
            if let Some(target) = look_at {
                avatar.set_look_at_target(target);
            }
            if let Some(weights) = visemes {
                avatar.apply_viseme(weights);
            }
            avatar.tick(dt);
        }
        let frame = gpu::acquire_frame(&self.surface).map_err(OverlayError::Surface)?;
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("ene-stage.overlay"),
            });
        if let Some(avatar) = self.avatar.as_mut() {
            avatar.render_to_texture(
                &gpu.queue,
                &mut encoder,
                &view,
                &self.depth_view,
                self.config.width,
                self.config.height,
            )?;
        } else {
            let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("ene-stage.overlay.clear"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: None,
                occlusion_query_set: None,
                timestamp_writes: None,
                multiview_mask: None,
            });
        }
        gpu.queue.submit(std::iter::once(encoder.finish()));
        frame.present();
        Ok(())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum OverlayError {
    #[error("surface: {0}")]
    Surface(String),
    #[error(transparent)]
    Avatar(#[from] crate::avatar::AvatarError),
}
