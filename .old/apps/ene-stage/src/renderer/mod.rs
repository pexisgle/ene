//! Overlay GPU path: VRM, optional Slint offscreen, premul compositor.
//!
//! This module owns drawing only. Window-level hit-test and interaction
//! mode live in [`crate::interaction_controller`].

mod compositor;
pub mod slint_gpu;

use ene_vrm::DebugRenderer;
use wgpu::{Texture, TextureView};
use winit::dpi::PhysicalSize;

use crate::avatar::{AvatarError, CompanionAvatar};
use crate::gpu::GpuContext;
use compositor::PremulCompositor;
use slint_gpu::SlintOverlayLayer;

/// GPU resources that draw one overlay surface.
pub struct StageRenderer {
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    format: wgpu::TextureFormat,
    depth: Texture,
    depth_view: TextureView,
    debug: DebugRenderer,
    compositor: PremulCompositor,
    ui_target: Option<(Texture, TextureView, (u32, u32))>,
}

impl StageRenderer {
    #[must_use]
    pub fn new(
        gpu: &GpuContext,
        surface: wgpu::Surface<'static>,
        config: wgpu::SurfaceConfiguration,
        format: wgpu::TextureFormat,
        depth: Texture,
        depth_view: TextureView,
    ) -> Self {
        let compositor = PremulCompositor::new(&gpu.device, config.format);
        Self {
            surface,
            config,
            format,
            depth,
            depth_view,
            debug: DebugRenderer::new(&gpu.device, format),
            compositor,
            ui_target: None,
        }
    }

    #[must_use]
    pub const fn format(&self) -> wgpu::TextureFormat {
        self.format
    }

    #[must_use]
    pub const fn config(&self) -> &wgpu::SurfaceConfiguration {
        &self.config
    }

    #[must_use]
    pub const fn size(&self) -> PhysicalSize<u32> {
        PhysicalSize::new(self.config.width, self.config.height)
    }

    pub fn resize(&mut self, gpu: &GpuContext, new_size: PhysicalSize<u32>) {
        if new_size.width == 0 || new_size.height == 0 {
            return;
        }
        self.config.width = new_size.width;
        self.config.height = new_size.height;
        self.surface.configure(&gpu.device, &self.config);
        let (depth, depth_view) =
            crate::gpu::create_depth(&gpu.device, new_size.width, new_size.height);
        self.depth = depth;
        self.depth_view = depth_view;
        self.ui_target = None;
    }

    pub fn reconfigure_surface(&mut self, gpu: &GpuContext) {
        self.surface.configure(&gpu.device, &self.config);
        let (depth, depth_view) =
            crate::gpu::create_depth(&gpu.device, self.config.width, self.config.height);
        self.depth = depth;
        self.depth_view = depth_view;
        self.ui_target = None;
    }

    fn ensure_ui_target(&mut self, gpu: &GpuContext) -> Option<(u32, u32)> {
        let size = (self.config.width, self.config.height);
        if size.0 == 0 || size.1 == 0 {
            return None;
        }
        let recreate = match &self.ui_target {
            Some((_, _, existing)) => *existing != size,
            None => true,
        };
        if recreate {
            let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("ene-stage-slint-offscreen"),
                size: wgpu::Extent3d {
                    width: size.0,
                    height: size.1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            self.ui_target = Some((texture, view, size));
        }
        Some(size)
    }

    /// Draw VRM (and optional Slint overlay) onto the swapchain.
    pub fn render(
        &mut self,
        gpu: &GpuContext,
        avatars: &mut [&mut CompanionAvatar],
        collider_debug: bool,
        highlight: Option<usize>,
        slint_layer: Option<&SlintOverlayLayer>,
    ) -> Result<(), AvatarError> {
        let frame = match crate::gpu::acquire_frame(&self.surface) {
            Ok(frame) => frame,
            Err(err) => {
                if err.contains("lost") || err.contains("outdated") {
                    tracing::warn!(error = %err, "surface lost or outdated; reconfiguring");
                    self.reconfigure_surface(gpu);
                    return Ok(());
                }
                tracing::debug!(error = %err, "surface skipped");
                return Ok(());
            }
        };
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("ene-stage.overlay"),
            });
        if avatars.is_empty() {
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
        } else {
            for (index, avatar) in avatars.iter_mut().enumerate() {
                avatar.render_to_texture(
                    &gpu.queue,
                    &mut encoder,
                    &view,
                    &self.depth_view,
                    self.config.width,
                    self.config.height,
                    index == 0,
                )?;
            }
        }
        if collider_debug || highlight.is_some() {
            self.debug.clear();
            if collider_debug {
                for avatar in avatars.iter() {
                    avatar.push_spring_collider_wires(&mut self.debug);
                    avatar.push_part_collider_wires(&mut self.debug);
                }
            }
            if let Some(index) = highlight
                && let Some(avatar) = avatars.get(index)
            {
                avatar.push_interaction_outline(&mut self.debug);
            }
            let camera_uniform = avatars
                .first()
                .and_then(|avatar| avatar.debug_camera_uniform());
            if let Some(camera_uniform) = camera_uniform {
                self.debug.render(
                    &gpu.device,
                    &gpu.queue,
                    &mut encoder,
                    &view,
                    &self.depth_view,
                    &camera_uniform,
                );
            }
        }

        let slint_wrote = if let Some(layer) = slint_layer {
            if let Some(size) = self.ensure_ui_target(gpu) {
                if layer.size() == size {
                    if let Some((_, ui_view, _)) = self.ui_target.as_ref() {
                        layer.render(&gpu.device, &gpu.queue, ui_view)
                    } else {
                        false
                    }
                } else {
                    false
                }
            } else {
                false
            }
        } else {
            false
        };
        if slint_wrote && let Some((_, ui_view, _)) = self.ui_target.as_ref() {
            self.compositor
                .encode(&mut encoder, ui_view, &view, &gpu.device);
        }

        gpu.queue.submit(std::iter::once(encoder.finish()));
        frame.present();
        Ok(())
    }
}
