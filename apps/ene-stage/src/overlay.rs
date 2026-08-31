//! Transparent always-on-top character overlay (wgpu, no egui).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use glam::Vec3;
use winit::dpi::PhysicalSize;
use winit::window::{Window, WindowId};

use crate::avatar::CompanionAvatar;
use crate::gpu::{self, GpuContext, GpuError};
use crate::renderer::StageRenderer;
use crate::renderer::slint_gpu::SlintOverlayLayer;

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

/// One avatar that could not be loaded while another avatar was available.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AvatarLoadFailure {
    pub soul_id: String,
    pub error: String,
}

/// Outcome of loading the selected overlay avatars.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AvatarLoadReport {
    pub loaded: usize,
    pub failures: Vec<AvatarLoadFailure>,
}

/// Native overlay window that draws VRM into a transparent swapchain.
pub struct OverlayWindow {
    pub window: Arc<Window>,
    renderer: StageRenderer,
    pub slots: Vec<OverlaySlot>,
    pub transparent: bool,
    pub transparency_supported: bool,
    pub click_through: bool,
    pub collider_debug: bool,
    slint_layer: Option<SlintOverlayLayer>,
    active_soul_id: Option<String>,
    hover_soul_id: Option<String>,
    drag_soul_id: Option<String>,
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
            renderer: StageRenderer::new(gpu, surface, config, format, depth, depth_view),
            slots: Vec::new(),
            transparent: transparent && transparency_supported,
            transparency_supported,
            click_through: transparent && transparency_supported,
            collider_debug: false,
            slint_layer: None,
            last_frame: Instant::now(),
            active_soul_id: None,
            hover_soul_id: None,
            drag_soul_id: None,
        })
    }

    #[must_use]
    pub fn id(&self) -> WindowId {
        self.window.id()
    }

    #[must_use]
    pub fn format(&self) -> wgpu::TextureFormat {
        self.renderer.format()
    }

    pub fn set_slint_layer(&mut self, layer: Option<SlintOverlayLayer>) {
        self.slint_layer = layer;
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
        self.renderer.resize(gpu, size);
        if let Some(layer) = &mut self.slint_layer {
            *layer = SlintOverlayLayer::new(size.width, size.height);
        }
    }

    pub fn set_click_through(&mut self, enabled: bool) {
        self.click_through = enabled;
    }

    /// Chrome on (decorations visible) always hit-tests so Allow/Detail work.
    /// Chrome off restores the saved click-through preference.
    /// Hit-test OS calls go through [`crate::interaction_controller::StageInteractionController`].
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
    ) -> Result<AvatarLoadReport, crate::avatar::AvatarError> {
        let mut loaded = Vec::new();
        let mut failures = Vec::new();
        let mut last_err = None;
        for spec in specs {
            match load_one(
                gpu,
                self.renderer.format(),
                &spec.path,
                spec.motions_dir.as_deref(),
            ) {
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
                    failures.push(AvatarLoadFailure {
                        soul_id: spec.soul_id.clone(),
                        error: err.to_string(),
                    });
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
        Ok(AvatarLoadReport {
            loaded: self.slots.len(),
            failures,
        })
    }

    pub fn set_interaction_targets(
        &mut self,
        active_soul_id: Option<&str>,
        hover_soul_id: Option<&str>,
        drag_soul_id: Option<&str>,
    ) {
        self.active_soul_id = active_soul_id.map(ToOwned::to_owned);
        self.hover_soul_id = hover_soul_id.map(ToOwned::to_owned);
        self.drag_soul_id = drag_soul_id.map(ToOwned::to_owned);
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
        let highlight =
            highlight_soul.and_then(|soul| self.slots.iter().position(|slot| slot.soul_id == soul));
        let mut avatars: Vec<&mut CompanionAvatar> =
            self.slots.iter_mut().map(|slot| &mut slot.avatar).collect();
        self.renderer
            .render(
                gpu,
                avatars.as_mut_slice(),
                self.collider_debug,
                highlight,
                self.slint_layer.as_ref(),
            )
            .map_err(OverlayError::Avatar)
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
