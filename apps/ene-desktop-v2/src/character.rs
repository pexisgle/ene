//! Character rendering for the v2 desktop app.
//!
//! PR3 hosts the [`CharacterRenderer`], which:
//!
//! - Loads the default `.vrm` (currently the `AliciaSolid.vrm`
//!   bundled in `assets/characters/Alicia/`) on first
//!   initialisation, falling back to a synthetic 1-triangle debug
//!   model if the file is missing.
//! - Owns the wgpu depth texture matching the character window's
//!   surface size.
//! - Exposes [`CharacterRenderer::render`], which the
//!   [`Runtime`](crate::runtime::Runtime) calls every frame from
//!   `RedrawRequested`.
//!
//! PR4 will swap the hard-coded default for the user-selected
//! character from `CharacterSettings::current_character()` and
//! consume the `EmotionQueue` to drive morph targets.
use std::path::PathBuf;

use ene_vrm::{ModelUniform, OrthographicCamera, VrmModel, VrmRenderer, load_vrm};
use glam::Vec3;

use crate::look_at::{LookAtState, compute_world_target};

/// Owns the loaded [`VrmModel`] and its [`VrmRenderer`].
///
/// Construction is infallible at the type level — if the default VRM
/// is missing the renderer produces a synthetic fallback so the
/// window still clears correctly.
pub struct CharacterRenderer {
    /// `None` if loading the default VRM failed; the renderer still
    /// draws the clear color but no geometry.
    model: Option<VrmModel>,
    /// `None` if no model loaded (matches `model.is_none()`).
    renderer: Option<VrmRenderer>,
    /// Orthographic camera (PR3 has a fixed position; PR4 will let
    /// the user pan).
    camera: OrthographicCamera,
    /// Depth texture (matches the character window's surface size).
    depth_view: Option<wgpu::TextureView>,
    /// The depth texture's *current* size, so the renderer can
    /// rebuild the view when the window resizes.
    depth_size: (u32, u32),
    /// Default VRM path (resolved at construction time).
    default_vrm: Option<PathBuf>,
    /// PR4.2: cursor → smoothed world target state.
    look_at: LookAtState,
}

impl CharacterRenderer {
    /// Build an un-initialized renderer. The runtime calls
    /// [`CharacterRenderer::init`] once it has the actual surface
    /// format.
    pub fn uninit(assets_dir: &std::path::Path, default_vrm: &str) -> Self {
        Self {
            model: None,
            renderer: None,
            camera: OrthographicCamera::default(),
            depth_view: None,
            depth_size: (0, 0),
            default_vrm: Some(assets_dir.join(default_vrm)),
            look_at: LookAtState::default(),
        }
    }

    /// Load the default VRM and build the render pipeline. Safe to
    /// call more than once; subsequent calls rebuild the pipeline
    /// (e.g. after a surface-format change).
    pub fn init(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        surface_format: wgpu::TextureFormat,
    ) {
        let Some(default_vrm) = self.default_vrm.clone() else {
            return;
        };
        match load_vrm(&default_vrm, device, queue) {
            Ok(model) => {
                let prim_count: usize = model.meshes.iter().map(|m| m.primitives.len()).sum();
                let total_vertices: u32 = model
                    .meshes
                    .iter()
                    .flat_map(|m| m.primitives.iter())
                    .map(|p| p.vertex_count)
                    .sum();
                let total_indices: u32 = model
                    .meshes
                    .iter()
                    .flat_map(|m| m.primitives.iter())
                    .map(|p| p.index_count)
                    .sum();
                let textured = model
                    .meshes
                    .iter()
                    .flat_map(|m| m.primitives.iter())
                    .filter(|p| p.base_color.is_some())
                    .count();
                tracing::info!(
                    "Loaded VRM {}: {} meshes, {} primitives, {} vertices, {} indices, {} with base-color, {} joints",
                    default_vrm.display(),
                    model.meshes.len(),
                    prim_count,
                    total_vertices,
                    total_indices,
                    textured,
                    model.joint_count(),
                );
                let renderer = VrmRenderer::new(device, queue, surface_format, &model);
                self.model = Some(model);
                self.renderer = Some(renderer);
            }
            Err(err) => {
                tracing::warn!(
                    "Failed to load default VRM {}: {err}; character window will be blank",
                    default_vrm.display()
                );
            }
        }
    }

    /// Update the depth texture to match the surface size. Call this
    /// from the `Resized` / `ScaleFactorChanged` handlers.
    pub fn resize(&mut self, device: &wgpu::Device, size: (u32, u32)) {
        if size.0 == 0 || size.1 == 0 || size == self.depth_size {
            return;
        }
        let depth = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("character.depth"),
            size: wgpu::Extent3d {
                width: size.0,
                height: size.1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth32Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        self.depth_view = Some(depth.create_view(&wgpu::TextureViewDescriptor::default()));
        self.depth_size = size;
        self.camera.set_aspect(size.0 as f32 / size.1 as f32);
    }

    /// Draw the model into `view`. No-op if the model failed to load.
    /// `model_uniform` is composed by the runtime every frame from
    /// `CharacterState` (position + scale) and applied between
    /// view-proj and the vertex position in the shader.
    pub fn render(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        view: &wgpu::TextureView,
        transparent: bool,
        model_uniform: &ModelUniform,
    ) {
        let (Some(model), Some(renderer), Some(depth_view)) =
            (&self.model, &self.renderer, &self.depth_view)
        else {
            // Even with no model, make sure we clear the surface.
            self.clear_only(device, queue, view, transparent);
            return;
        };

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("character.encoder"),
        });
        renderer.render(
            queue,
            &mut encoder,
            view,
            depth_view,
            model,
            &self.camera,
            model_uniform,
            transparent,
        );
        queue.submit(std::iter::once(encoder.finish()));
    }

    /// Clear-only path used when the model failed to load.
    fn clear_only(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        view: &wgpu::TextureView,
        transparent: bool,
    ) {
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("character.clear_only"),
        });
        let (gray, alpha) = if transparent { (0.0, 0.0) } else { (0.2, 1.0) };
        let _ = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("character.clear_only.pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: gray,
                        g: gray,
                        b: gray,
                        a: alpha,
                    }),
                    store: wgpu::StoreOp::Store,
                },
                depth_slice: None,
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        queue.submit(std::iter::once(encoder.finish()));
    }

    /// Path to the VRM that was loaded (or attempted). Useful for
    /// PR4's character-switch logic.
    #[expect(dead_code)] // PR4 will read this when wiring per-character selection.
    pub fn default_vrm_path(&self) -> Option<&std::path::Path> {
        self.default_vrm.as_deref()
    }

    /// PR4.2: update the cursor-driven head-look-at state.
    ///
    /// `head_world` is the world-space position of the character's
    /// head, derived from `character_state.character_position` plus
    /// the model's head offset. The smoothed world target is stored
    /// in [`LookAtState`] and exposed via
    /// [`CharacterRenderer::look_at_target`]. PR4.5+ (skinning) will
    /// consume the target to rotate the humanoid bones; until then
    /// the runtime may feed the target into the orthographic camera
    /// to give a subtle pan.
    pub fn update_look_at(
        &mut self,
        cursor_logical: glam::Vec2,
        viewport_size: (u32, u32),
        head_world: Vec3,
        strength: f32,
        dt_secs: f32,
    ) -> Vec3 {
        let eye = ene_vrm::camera::DEFAULT_EYE.into();
        let target = ene_vrm::camera::DEFAULT_TARGET.into();
        let up = ene_vrm::camera::DEFAULT_UP.into();
        compute_world_target(
            cursor_logical,
            viewport_size,
            eye,
            target,
            up,
            head_world,
            strength,
            &mut self.look_at,
            dt_secs,
        )
    }

    /// The most recent smoothed world target (or `None` if no cursor
    /// sample has been processed yet).
    #[expect(dead_code)] // PR4.5+ skinning will read this to drive bone rotations.
    pub fn look_at_target(&self) -> Option<Vec3> {
        self.look_at.smoothed_world_target
    }

    /// Mutable access to the underlying [`LookAtState`]. Used by the
    /// runtime to compute `body_tracking_for_strength` for the
    /// current `look_at_strength` slider value.
    #[expect(dead_code)] // PR4.5+ will read body-tracking profile.
    pub fn body_tracking(&self, strength: f32) -> crate::look_at::BodyTracking {
        crate::look_at::body_tracking_for_strength(strength)
    }
}
