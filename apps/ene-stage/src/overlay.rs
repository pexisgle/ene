//! Transparent always-on-top character overlay (wgpu, no egui).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use ene_vrm::DebugRenderer;
use glam::Vec3;
use winit::dpi::PhysicalSize;
use winit::window::{Window, WindowId};

use crate::avatar::CompanionAvatar;
use crate::gpu::{self, GpuContext, GpuError};

/// One GPU-resident body drawn in the overlay, keyed by soul.
pub struct OverlaySlot {
    pub soul_id: String,
    pub avatar: CompanionAvatar,
}

/// Path + motions for one occupant the overlay should load.
pub struct AvatarLoad {
    pub soul_id: String,
    pub path: PathBuf,
    pub motions_dir: Option<PathBuf>,
}

/// Native overlay window that draws VRM into a transparent swapchain.
pub struct OverlayWindow {
    pub window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    format: wgpu::TextureFormat,
    depth: wgpu::Texture,
    depth_view: wgpu::TextureView,
    pub slots: Vec<OverlaySlot>,
    pub transparent: bool,
    pub transparency_supported: bool,
    pub click_through: bool,
    pub collider_debug: bool,
    debug: DebugRenderer,
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
        let capabilities = surface.get_capabilities(&gpu.adapter);
        let transparency_supported = gpu::alpha_mode_supports_transparency(alpha);
        let adapter_info = gpu.adapter.get_info();
        tracing::info!(
            backend = ?adapter_info.backend,
            adapter = %adapter_info.name,
            format = ?format,
            alpha_mode = ?alpha,
            supported_alpha_modes = ?capabilities.alpha_modes,
            transparent_requested = transparent,
            transparency_supported,
            "overlay surface configured"
        );
        if transparent && !transparency_supported {
            tracing::warn!(
                alpha_mode = ?alpha,
                supported_alpha_modes = ?capabilities.alpha_modes,
                "transparent overlay unavailable; hiding avatar window"
            );
            window.set_visible(false);
        }
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
            slots: Vec::new(),
            transparent: transparent && transparency_supported,
            transparency_supported,
            click_through: transparent && transparency_supported,
            collider_debug: false,
            debug: DebugRenderer::new(&gpu.device, format),
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

    #[must_use]
    pub fn has_avatars(&self) -> bool {
        !self.slots.is_empty()
    }

    #[must_use]
    pub fn first_avatar(&self) -> Option<&CompanionAvatar> {
        self.slots.first().map(|slot| &slot.avatar)
    }

    pub fn first_avatar_mut(&mut self) -> Option<&mut CompanionAvatar> {
        self.slots.first_mut().map(|slot| &mut slot.avatar)
    }

    pub fn avatar_mut(&mut self, soul_id: &str) -> Option<&mut CompanionAvatar> {
        self.slots
            .iter_mut()
            .find(|slot| slot.soul_id == soul_id)
            .map(|slot| &mut slot.avatar)
    }

    pub fn avatar_or_first_mut(&mut self, soul_id: &str) -> Option<&mut CompanionAvatar> {
        if self.slots.iter().any(|slot| slot.soul_id == soul_id) {
            self.avatar_mut(soul_id)
        } else {
            self.first_avatar_mut()
        }
    }

    pub fn reset_visemes(&mut self) {
        let silence = ene_vrm::VisemeWeights::default();
        for slot in &mut self.slots {
            slot.avatar.apply_viseme(silence);
        }
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
        if self.click_through != enabled {
            self.click_through = enabled;
            if let Err(err) = self.window.set_cursor_hittest(!enabled) {
                tracing::debug!(error = %err, "cursor hittest unsupported");
            }
        }
    }

    /// Chrome on (decorations visible) always hit-tests so Allow/Detail work.
    /// Chrome off restores the saved click-through preference.
    pub fn apply_click_through(&mut self, preferred: bool) {
        self.set_click_through(self.transparent && preferred);
    }

    pub fn toggle_chrome(&mut self) {
        self.transparent = !self.transparent;
        let inner = self.window.inner_size();
        self.window.set_decorations(!self.transparent);
        self.window.set_transparent(self.transparent);
        if self.window.request_inner_size(inner).is_none() {
            tracing::debug!("overlay inner-size request deferred until the next scale event");
        }
    }

    pub fn clear_avatars(&mut self) {
        self.slots.clear();
    }

    pub fn load_avatars(
        &mut self,
        gpu: &GpuContext,
        specs: &[AvatarLoad],
    ) -> Result<usize, crate::avatar::AvatarError> {
        let mut loaded = Vec::new();
        let mut last_err = None;
        for spec in specs {
            match load_one(gpu, self.format, &spec.path, spec.motions_dir.as_deref()) {
                Ok(avatar) => loaded.push(OverlaySlot {
                    soul_id: spec.soul_id.clone(),
                    avatar,
                }),
                Err(err) => {
                    tracing::warn!(
                        error = %err,
                        path = %spec.path.display(),
                        soul_id = %spec.soul_id,
                        "VRM load failed"
                    );
                    last_err = Some(err);
                }
            }
        }
        self.slots = loaded;
        if self.slots.is_empty() {
            return Err(last_err.unwrap_or_else(|| {
                crate::avatar::AvatarError::Io(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "no avatar paths",
                ))
            }));
        }
        Ok(self.slots.len())
    }

    pub fn tick_and_render(
        &mut self,
        gpu: &GpuContext,
        look_at: Option<Vec3>,
        visemes: Option<ene_vrm::VisemeWeights>,
        speaking_soul: Option<&str>,
        highlight_soul: Option<&str>,
    ) -> Result<(), OverlayError> {
        let now = Instant::now();
        let dt = now.saturating_duration_since(self.last_frame).as_secs_f32();
        self.last_frame = now;
        for slot in &mut self.slots {
            if let Some(target) = look_at {
                slot.avatar.set_look_at_target(target);
            }
            if speaking_soul.is_some_and(|id| id == slot.soul_id)
                && let Some(weights) = visemes
            {
                slot.avatar.apply_viseme(weights);
            }
            slot.avatar.tick(dt);
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
        if self.slots.is_empty() {
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
            for (index, slot) in self.slots.iter_mut().enumerate() {
                slot.avatar.render_to_texture(
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
        if self.collider_debug || highlight_soul.is_some() {
            self.debug.clear();
            if self.collider_debug {
                for slot in &self.slots {
                    slot.avatar.push_spring_collider_wires(&mut self.debug);
                }
            }
            if let Some(highlight_soul) = highlight_soul {
                for slot in &self.slots {
                    if slot.soul_id == highlight_soul {
                        slot.avatar.push_interaction_outline(&mut self.debug);
                    }
                }
            }
            let camera_uniform = self
                .slots
                .first()
                .and_then(|slot| slot.avatar.debug_camera_uniform());
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
        gpu.queue.submit(std::iter::once(encoder.finish()));
        frame.present();
        Ok(())
    }
}

fn load_one(
    gpu: &GpuContext,
    format: wgpu::TextureFormat,
    path: &Path,
    motions_dir: Option<&Path>,
) -> Result<CompanionAvatar, crate::avatar::AvatarError> {
    let mut avatar = CompanionAvatar::load(path, &gpu.device, &gpu.queue, format)?;
    if let Some(dir) = motions_dir {
        avatar.load_motions(dir);
    }
    Ok(avatar)
}

#[derive(Debug, thiserror::Error)]
pub enum OverlayError {
    #[error("surface: {0}")]
    Surface(String),
    #[error(transparent)]
    Avatar(#[from] crate::avatar::AvatarError),
}
