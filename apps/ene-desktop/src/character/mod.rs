//! Character rendering. Owns the loaded VRM, depth texture, the
//! orthographic camera, and per-frame state (look-at, drag, motion
//! playback, spring bones, FXAA). The runtime calls
//! [`CharacterRenderer::render`] every `RedrawRequested`.
use std::path::PathBuf;

use crate::settings::AntialiasingMode;
use ene_vrm::{
    ExpressionName, HumanoidBoneEntry, LookAtBoneOutput, LookAtEvaluator, LookAtOutput,
    LookAtProperties, ModelUniform, OrthographicCamera, VrmModel, VrmRenderer, evaluate_clip,
    load_vrm, load_vrma,
};
use ene_vrm::{
    SpringBoneProperties, SpringBoneSimulator, VrmaAsset, VrmaPlayer, post_process::PostProcessor,
};
use glam::{Mat4, Quat, Vec3};

use crate::look_at::{LookAtState, compute_world_target, head_world_for};

pub mod collider;
pub mod drag;

pub use collider::{BonePose, BoneShapeSpec};
pub use drag::CharacterDragState;

/// Owns the loaded [`VrmModel`] and its [`VrmRenderer`]. The
/// renderer is built via [`CharacterRenderer::uninit`] (which
/// never touches the GPU) and then [`CharacterRenderer::init`]
/// once the surface format is known. If the default VRM is
/// missing the renderer is left in the empty state and the
/// window only clears.
pub struct CharacterRenderer {
    model: Option<VrmModel>,
    renderer: Option<VrmRenderer>,
    camera: OrthographicCamera,
    depth_view: Option<wgpu::TextureView>,
    depth_size: (u32, u32),
    default_vrm: Option<PathBuf>,
    look_at: LookAtState,
    /// Per-bone `LookAt` output for `"bone"`-type models.
    look_at_bone_output: Option<LookAtBoneOutput>,
    pub drag: CharacterDragState,
    vrma: Option<VrmaAsset>,
    vrma_player: VrmaPlayer,
    vrma_path: Option<PathBuf>,
    /// Resolved asset directory for motion clip lookups (#133).
    assets_dir: Option<PathBuf>,
    active_bone_nodes: Vec<usize>,
    /// Spring-bone simulator. `None` for models without `VRMC_springBone`.
    spring_bone_sim: Option<SpringBoneSimulator>,
    /// Cached `VRMC_springBone` properties. Cloned to avoid a
    /// borrow fight with `&mut VrmModel` in the per-frame update.
    spring_bone_props: Option<SpringBoneProperties>,
    /// FXAA post-processor. Rebuilt by
    /// [`CharacterRenderer::set_antialiasing_mode`].
    post_processor: Option<PostProcessor>,
    /// Cached FXAA shader module.
    fxaa_shader: Option<wgpu::ShaderModule>,
    /// Surface format the post-processor was built against.
    fxaa_format: Option<wgpu::TextureFormat>,
}

impl CharacterRenderer {
    /// Build an un-initialized renderer. The runtime calls
    /// [`CharacterRenderer::init`] once the surface format is
    /// known.
    pub fn uninit(assets_dir: &std::path::Path, default_vrm: &str) -> Self {
        Self {
            model: None,
            renderer: None,
            camera: OrthographicCamera::default(),
            depth_view: None,
            depth_size: (0, 0),
            default_vrm: Some(assets_dir.join(default_vrm)),
            look_at: LookAtState::default(),
            look_at_bone_output: None,
            drag: CharacterDragState::default(),
            spring_bone_sim: None,
            spring_bone_props: None,
            post_processor: None,
            fxaa_shader: None,
            fxaa_format: None,
            vrma: None,
            vrma_player: VrmaPlayer::default(),
            vrma_path: None,
            assets_dir: Some(assets_dir.to_path_buf()),
            active_bone_nodes: Vec::new(),
        }
    }

    /// Test-only: install a fully-built `VrmModel` directly on
    /// the renderer. Bypasses `init`'s `load_vrm` + `wgpu` path
    /// so per-frame CPU math can be unit-tested without a device.
    #[cfg(test)]
    pub(crate) fn set_model_for_test(&mut self, model: VrmModel) {
        if let Some(props) = model.spring_bones.clone() {
            self.spring_bone_sim = build_spring_bone_simulator(&model, &props);
            self.spring_bone_props = Some(props);
        } else {
            self.spring_bone_sim = None;
            self.spring_bone_props = None;
        }
        self.model = Some(model);
    }

    /// Load the default VRM and build the render pipeline. Safe to
    /// call more than once; subsequent calls rebuild the pipeline
    /// (e.g. after a surface-format change).
    pub fn init(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        surface_format: wgpu::TextureFormat,
        mask_format: Option<wgpu::TextureFormat>,
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
                let renderer = VrmRenderer::new(device, queue, surface_format, mask_format, &model);
                // Build the spring-bone simulator before moving the
                // model into `self.model` so the borrow on
                // `model.nodes.*` is clean.
                if let Some(props) = model.spring_bones.clone() {
                    let sim = build_spring_bone_simulator(&model, &props);
                    self.spring_bone_sim = sim;
                    self.spring_bone_props = Some(props);
                } else {
                    self.spring_bone_sim = None;
                    self.spring_bone_props = None;
                }
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

    /// Look up a motion clip by name and load it from
    /// `{assets_dir}/motions/{name}.vrma`.
    pub fn play_motion_by_name(&mut self, name: &str) -> Result<(), String> {
        let Some(ref assets_dir) = self.assets_dir else {
            return Err("assets_dir not set".into());
        };
        let path = assets_dir.join("motions").join(format!("{name}.vrma"));
        self.play_motion(&path);
        self.vrma
            .as_ref()
            .map(|_| ())
            .ok_or_else(|| format!("motion '{name}' failed to load"))
    }

    /// Returns the name (file stem) of the currently-playing motion, if any.
    pub fn active_motion_name(&self) -> Option<&str> {
        self.vrma_path
            .as_ref()
            .and_then(|p| p.file_stem())
            .and_then(|s| s.to_str())
    }

    /// Load a `.vrma` from disk and store the asset. Safe to call
    /// before the model is loaded. Errors are logged and the
    /// previous motion is kept.
    pub fn play_motion(&mut self, vrma_path: &std::path::Path) {
        match load_vrma(vrma_path) {
            Ok(asset) => {
                tracing::info!(
                    "Loaded VRMA {} ({} clip(s))",
                    vrma_path.display(),
                    asset.clips.len()
                );
                self.vrma_player = VrmaPlayer {
                    time: 0.0,
                    speed: 1.0,
                    playing: true,
                    repeat: VrmaPlayer::default().repeat,
                };
                self.vrma = Some(asset);
                self.vrma_path = Some(vrma_path.to_path_buf());
            }
            Err(err) => {
                tracing::warn!(
                    "Failed to load VRMA {}: {err}; model will stay in rest pose",
                    vrma_path.display()
                );
            }
        }
    }

    /// Advance the `VrmaPlayer` by `dt_secs`, evaluate the active
    /// clip, push expression weights into the morph layer, step
    /// the spring-bone simulator, and recompute the skin palette.
    /// Returns the new palette for the runtime to forward to
    /// [`VrmRenderer::update_skin_palette`]. `None` when there is
    /// no motion, no model, or the player is paused.
    pub fn update_motion(&mut self, dt_secs: f32) -> Option<Vec<glam::Mat4>> {
        let model = self.model.as_mut()?;

        let mut frame = if self.vrma_player.playing {
            if let Some(asset) = &self.vrma
                && let Some(clip) = asset.clips.first()
            {
                self.vrma_player.advance(dt_secs, clip.duration);
                evaluate_clip(clip, self.vrma_player.time)
            } else {
                ene_vrm::VrmaFrame::default()
            }
        } else {
            ene_vrm::VrmaFrame::default()
        };

        // Retarget hips translation (convert absolute canonical position to a relative delta)
        if let Some(ref mut hips_trans) = frame.hips_translation {
            if let Some(asset) = &self.vrma
                && let Some(&src_hips_node) = asset.properties.humanoid_bones.get("hips")
                && let Some(dst_hips_entry) = model.humanoid.hips()
            {
                let src_rest_local = asset.node_rest_positions[src_hips_node];
                let src_rest_global_y = asset.node_world_rest_positions[src_hips_node].y;
                let dst_rest_global_y = bone_world_rest_position(model, dst_hips_entry).y;

                let delta = *hips_trans - src_rest_local;
                let scale = if src_rest_global_y.abs() < 1e-6 {
                    1.0
                } else {
                    dst_rest_global_y / src_rest_global_y
                };
                *hips_trans = delta * scale;
            } else {
                frame.hips_translation = None;
            }
        }

        // Expression weights (morph targets).
        if !frame.expression_weights.is_empty() {
            let expressions_meta = model.expressions_meta.clone();
            let layer_mut = model.expressions_mut();
            for (name, weight) in &frame.expression_weights {
                let name = ene_vrm::ExpressionName::new(name.clone());
                layer_mut.set_expression(&name, *weight);
            }
            layer_mut.apply_overrides(&expressions_meta);
        }

        // Compose the LookAt bone output (set by
        // `update_look_at` for `"bone"`-type models) on top of
        // the VRMA pose so the head and eyes track the cursor.
        // `None` for `"expression"`-type models, where the
        // LookAt signal routes into morph weights via
        // `apply_emotions` instead.
        let palette = model.update_skin_palette(&frame, self.look_at_bone_output.as_ref());

        // Step the spring-bone simulator. The simulator reads
        // the per-node world transforms the palette update
        // just produced and writes the updated local
        // rotations for the affected joints back into
        // `model.nodes.local_rotations`. The next
        // `update_skin_palette` call (next frame) picks them
        // up, so the simulator's effect on the silhouette
        // lags one frame — a single-frame delay is
        // imperceptible at 60 Hz and is the standard pattern
        // for VRMC_springBone in v1 / v0 reference impls.
        if let (Some(sim), Some(props)) = (
            self.spring_bone_sim.as_mut(),
            self.spring_bone_props.as_ref(),
        ) {
            let n = model.nodes.len();
            let mut world_positions = std::collections::HashMap::with_capacity(n);
            let mut world_rotations = std::collections::HashMap::with_capacity(n);
            let mut parent_world_rotations = std::collections::HashMap::with_capacity(n);
            let mut collider_positions = std::collections::HashMap::new();
            let mut collider_rotations = std::collections::HashMap::new();
            for i in 0..n {
                world_positions.insert(i, model.nodes.world_positions[i]);
                world_rotations.insert(i, model.nodes.world_rotations[i]);
                let p = model.nodes.parents[i];
                parent_world_rotations.insert(
                    i,
                    if p < 0 {
                        Quat::IDENTITY
                    } else {
                        model.nodes.world_rotations[p as usize]
                    },
                );
            }
            for collider in &props.colliders {
                let node = collider.node;
                if node < n {
                    collider_positions.insert(node, model.nodes.world_positions[node]);
                    collider_rotations.insert(node, model.nodes.world_rotations[node]);
                }
            }
            let updated = sim.step(
                dt_secs,
                props,
                &world_positions,
                &world_rotations,
                &parent_world_rotations,
                &collider_positions,
                &collider_rotations,
            );
            for (node, rotation) in updated {
                if node < n {
                    model.nodes.local_rotations[node] = rotation;
                }
            }
        }

        if palette.is_empty() {
            None
        } else {
            Some(palette)
        }
    }

    /// Forward a new skin palette to the GPU. No-op when the
    /// renderer is missing or the palette is empty.
    pub fn update_skin_palette_gpu(&self, queue: &wgpu::Queue, palette: &[glam::Mat4]) {
        if let Some(renderer) = self.renderer.as_ref() {
            renderer.update_skin_palette(queue, palette);
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

    /// Draw the model into `view`. No-op if the model failed to
    /// load. `model_uniform` is composed by the runtime every
    /// frame from `CharacterState` (position + scale). When the
    /// AA mode is `Fxaa` and the post-processor is built, the
    /// model is drawn into its intermediate texture first, then
    /// the post-processor samples it into `view`.
    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        view: &wgpu::TextureView,
        transparent: bool,
        model_uniform: &ModelUniform,
        swapchain_size: (u32, u32),
        swapchain_format: wgpu::TextureFormat,
        aa_mode: AntialiasingMode,
    ) {
        // Lazily rebuild the post-processor BEFORE the model
        // borrow. The post-processor only touches
        // `self.post_processor` / `self.fxaa_shader` /
        // `self.fxaa_format`; the model / renderer / depth
        // view are read in the second half. Splitting the
        // two phases avoids a borrow conflict.
        if self.model.is_some() {
            if aa_mode == AntialiasingMode::Fxaa && self.post_processor.is_none() {
                self.try_build_post_processor(device, queue, swapchain_format, swapchain_size);
            } else if aa_mode != AntialiasingMode::Fxaa {
                self.post_processor = None;
            }
        }

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
        if let Some(post) = self.post_processor.as_mut() {
            let intermediate = post.intermediate_view();
            renderer.render(
                queue,
                &mut encoder,
                intermediate,
                depth_view,
                model,
                &self.camera,
                model_uniform,
                transparent,
            );
            post.render(device, queue, &mut encoder, view);
        } else {
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
        }
        queue.submit(std::iter::once(encoder.finish()));
    }

    /// Forward to the internal VRM renderer's `render_mask`.
    /// No-op if the renderer is uninitialised or was built without
    /// a `mask_format`.
    #[cfg_attr(target_os = "windows", expect(dead_code))]
    pub fn render_mask(
        &self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        model_uniform: &ModelUniform,
    ) {
        if let Some(r) = self.renderer.as_ref()
            && let Some(m) = self.model.as_ref()
            && let Ok(uniform) = self.camera.uniform()
        {
            r.render_mask(queue, encoder, view, m, &uniform, model_uniform);
        }
    }

    /// Lazily build the FXAA post-processor. Loads the FXAA
    /// shader (idempotent) and constructs a new `PostProcessor`
    /// for the given swapchain size and format.
    fn try_build_post_processor(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        format: wgpu::TextureFormat,
        swapchain_size: (u32, u32),
    ) {
        if swapchain_size.0 == 0 || swapchain_size.1 == 0 {
            return;
        }
        // The shader module is built once and re-used.
        if self.fxaa_shader.is_none() {
            let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("fxaa.shader"),
                source: wgpu::ShaderSource::Wgsl(
                    include_str!("../../../../crates/ene-vrm/src/shaders/fxaa.wgsl").into(),
                ),
            });
            self.fxaa_shader = Some(shader);
        }
        // Re-build when the swapchain format or size changes.
        let needs_rebuild = (self.fxaa_format != Some(format))
            || self
                .post_processor
                .as_ref()
                .is_none_or(|p| p.size() != swapchain_size);
        if !needs_rebuild {
            return;
        }
        if let Some(shader) = self.fxaa_shader.as_ref()
            && let Some(post) = PostProcessor::new(device, queue, format, swapchain_size, shader)
        {
            self.post_processor = Some(post);
            self.fxaa_format = Some(format);
        }
    }

    /// rebuild the post-processor at a new swapchain
    /// size. The runtime calls this on `WindowEvent::Resized`
    /// in lock-step with the depth-texture resize.
    pub fn resize_post_processor(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        swapchain_size: (u32, u32),
    ) {
        if let (Some(shader), Some(post)) =
            (self.fxaa_shader.as_ref(), self.post_processor.as_mut())
        {
            post.resize(device, queue, shader, swapchain_size);
        }
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

    /// Path to the VRM that was loaded (or attempted).
    #[expect(dead_code)]
    pub fn default_vrm_path(&self) -> Option<&std::path::Path> {
        self.default_vrm.as_deref()
    }

    /// Update the cursor-driven head-look-at state. The head
    /// world position is derived from the humanoid `head` bone
    /// (when present) so the look-at follows the model's actual
    /// head geometry, with a fallback to a 1 m Y offset for
    /// models without humanoid metadata. Writes the result to
    /// either the `ExpressionLayer` (for `"expression"`-type
    /// models) or [`CharacterRenderer::look_at_bone_output`]
    /// (for `"bone"`-type models).
    pub fn update_look_at(
        &mut self,
        cursor_logical: glam::Vec2,
        viewport_size: (u32, u32),
        character_position: Vec3,
        model_scale: f32,
        strength: f32,
        dt_secs: f32,
    ) -> Vec3 {
        let (head_world, head_rest_rotation) = self.head_world_for(character_position, model_scale);

        let props = self
            .model
            .as_ref()
            .and_then(|m| m.look_at().copied())
            .unwrap_or_default();
        let smoothing = LookAtProperties::DEFAULT_SMOOTHING;

        let eye = self.camera.eye().into();
        let target = self.camera.target().into();
        let up = ene_vrm::camera::DEFAULT_UP.into();
        let smoothed_target = compute_world_target(
            cursor_logical,
            viewport_size,
            eye,
            target,
            up,
            head_world,
            strength,
            &mut self.look_at,
            dt_secs,
            smoothing,
        );

        // Clone the expression metadata before mutating
        // `expressions_mut()` so the borrow checker stays
        // happy. A typical model has ~30 definitions, ~60 bytes
        // each — cheap to clone.
        let expressions_meta = self
            .model
            .as_ref()
            .map(|m| m.expressions_meta.clone())
            .unwrap_or_default();
        let evaluator = LookAtEvaluator::new(&props);
        match evaluator.evaluate(head_world, smoothed_target, head_rest_rotation) {
            LookAtOutput::Expression(e) => {
                if let Some(model) = self.model.as_mut() {
                    let layer = model.expressions_mut();
                    layer.set_expression(&ExpressionName::new("lookUp"), e.look_up);
                    layer.set_expression(&ExpressionName::new("lookDown"), e.look_down);
                    layer.set_expression(&ExpressionName::new("lookLeft"), e.look_left);
                    layer.set_expression(&ExpressionName::new("lookRight"), e.look_right);
                    layer.apply_overrides(&expressions_meta);
                }
                self.look_at_bone_output = None;
            }
            LookAtOutput::Bone(b) => {
                self.look_at_bone_output = Some(b);
            }
        }

        smoothed_target
    }

    /// Compute the world-space head position and rest rotation.
    /// Returns `(world_pos, rest_rotation)`. The rest rotation
    /// is `Quat::IDENTITY` when the model has no humanoid head
    /// bone.
    fn head_world_for(&self, character_position: Vec3, model_scale: f32) -> (Vec3, Quat) {
        let Some(model) = self.model.as_ref() else {
            return (head_world_for(character_position), Quat::IDENTITY);
        };
        let Some(head) = model.humanoid.head() else {
            return (head_world_for(character_position), Quat::IDENTITY);
        };
        let world = bone_world_position(
            bone_world_rest_position(model, head),
            model.center(),
            model.normalize_scale(),
            character_position,
            model_scale,
        );
        (world, head.rest.rotation)
    }

    /// Compute the world-space "body center" for camera
    /// targeting. Tries the humanoid `head` → `chest` → `hips`
    /// bones and falls back to `character_position` for models
    /// without humanoid bones. Used by
    /// [`Self::update_camera_target`].
    #[cfg_attr(not(test), expect(dead_code, reason = "Test-only helper"))]
    pub fn body_center_world(&self, character_position: Vec3, model_scale: f32) -> Vec3 {
        let Some(model) = self.model.as_ref() else {
            return character_position;
        };
        match pick_body_center_bone(&model.humanoid) {
            Some(bone) => bone_world_position(
                bone_world_rest_position(model, bone),
                model.center(),
                model.normalize_scale(),
                character_position,
                model_scale,
            ),
            None => character_position,
        }
    }

    /// Update the camera target to frame the character. The
    /// target is derived from the chest bone's **world rest Y**
    /// so it stays stable under animation — the animated world
    /// position would oscillate with an active VRMA motion and
    /// produce a choppy look-at.
    pub fn update_camera_target(&mut self, model_scale: f32) {
        let mut target = Vec3::ZERO;

        if let Some(model) = &self.model {
            let bone = model
                .humanoid
                .chest()
                .or_else(|| model.humanoid.by_name("upperChest"))
                .or_else(|| model.humanoid.by_name("spine"))
                .or_else(|| model.humanoid.hips());

            if let Some(bone) = bone {
                target.y += chest_target_y(
                    chest_world_rest_y(model, bone),
                    model.center()[1],
                    model.normalize_scale(),
                    model_scale,
                );
            }
        }

        let mut eye = target;
        eye.z += ene_vrm::camera::DEFAULT_EYE[2];
        eye.y += ene_vrm::camera::DEFAULT_EYE[1];

        self.camera.look_at(eye.into(), target.into());
    }

    /// Returns the current camera eye position.
    pub const fn camera_eye(&self) -> [f32; 3] {
        self.camera.eye()
    }

    /// Returns the current camera target position.
    pub const fn camera_target(&self) -> [f32; 3] {
        self.camera.target()
    }

    /// Per-frame camera uniform for the debug overlay so its
    /// `view_proj` matches the main VRM pass. Returns `Option`
    /// for API symmetry with the debug pipeline.
    pub fn camera_uniform_dbg(&self) -> Option<ene_vrm::camera::CameraUniform> {
        self.camera.uniform().ok()
    }

    /// Latest per-bone output for `"bone"`-type models. Consumed
    /// inside [`Self::update_motion`]; the accessor is kept for
    /// diagnostics.
    #[expect(dead_code)]
    pub const fn look_at_bone_output(&self) -> Option<&LookAtBoneOutput> {
        self.look_at_bone_output.as_ref()
    }

    /// The most recent smoothed world target (or `None`).
    #[expect(dead_code)]
    pub const fn look_at_target(&self) -> Option<Vec3> {
        self.look_at.smoothed_world_target
    }

    /// Per-bone body-tracking weights for the current
    /// `look_at_strength` slider value.
    #[expect(dead_code)]
    pub fn body_tracking(&self, strength: f32) -> crate::look_at::BodyTracking {
        crate::look_at::body_tracking_for_strength(strength)
    }

    /// Diagnostic: (`aspect_ratio`, eye, target, `viewport_height`).
    #[expect(dead_code)]
    pub const fn camera_dbg(&self) -> (f32, [f32; 3], [f32; 3], f32) {
        let (eye, target, viewport_height, aspect) = self.camera.debug();
        (aspect, eye, target, viewport_height)
    }

    /// Compute the auto-fit scale that makes the loaded model's
    /// normalised AABB fit the viewport with `margin` of unused
    /// space on every side. The runtime multiplies this by
    /// `settings.character_state.model_scale` so the user's zoom
    /// slider works on top of the fit. Returns `1.0` if no model
    /// is loaded.
    pub fn auto_fit_scale(&self, margin: f32) -> f32 {
        match self.model.as_ref() {
            None => 1.0,
            Some(model) => {
                let (lo, hi) = model.normalized_aabb();
                self.camera.compute_auto_fit_scale(lo, hi, margin)
            }
        }
    }

    /// Full per-character model matrix: folds the raw-space
    /// vertex buffer through `T(-center) * S(normalize_scale)`
    /// and the per-character translation / scale.
    pub fn model_matrix(&self, character_position: Vec3, actual_scale: f32) -> Mat4 {
        let (center, normalize_scale) = match self.model.as_ref() {
            None => ([0.0, 0.0, 0.0], 1.0),
            Some(m) => (m.center(), m.normalize_scale()),
        };
        Mat4::from_translation(character_position)
            * Mat4::from_scale(Vec3::splat(actual_scale))
            * Mat4::from_scale(Vec3::splat(normalize_scale))
            * Mat4::from_translation(Vec3::from(center) * -1.0)
    }

    /// Diagnostic: (`depth_width`, `depth_height`).
    #[expect(dead_code)]
    pub const fn depth_size_dbg(&self) -> (u32, u32) {
        self.depth_size
    }

    /// Hand the depth attachment to the debug overlay so it can
    /// `LoadOp::Load` the depth the main VRM pass wrote. Returns
    /// `None` before `init` / `resize` produces a depth texture.
    pub const fn depth_view(&self) -> Option<&wgpu::TextureView> {
        self.depth_view.as_ref()
    }

    pub const fn model(&self) -> Option<&VrmModel> {
        self.model.as_ref()
    }

    /// Mutable accessor for the loaded [`VrmModel`]. Used by the
    /// render-side emotion pipeline to apply expression weights
    /// (`expressions_mut().set_expression`) after
    /// `app.update()` has drained the `EmotionPipelineState`
    /// queue.
    pub const fn model_mut(&mut self) -> Option<&mut VrmModel> {
        self.model.as_mut()
    }

    /// Diagnostic: AABB of the loaded vertex data (min, max).
    /// The loader's normalize centres the AABB on origin; if
    /// not symmetric, that's a bug.
    #[expect(dead_code)]
    pub fn model_aabb_dbg(&self) -> Option<([f32; 3], [f32; 3])> {
        self.model.as_ref().map(ene_vrm::VrmModel::aabb)
    }

    /// Diagnostic: the loader-captured AABB centre. The
    /// runtime folds `T(-center)` into the model matrix; if the
    /// centre is wildly off (e.g. a hair vertex got included in
    /// the AABB) the model will be shifted out of the viewport.
    #[expect(dead_code)]
    pub fn model_dbg_center(&self) -> [f32; 3] {
        self.model
            .as_ref()
            .map_or([0.0; 3], ene_vrm::VrmModel::center)
    }

    /// Diagnostic: the loader's `1.5 / max_extent` scale.
    /// `actual_scale × normalize_scale` should be ~1.42 for
    /// Alicia; if the runtime sees a different value, the loader
    /// computed the wrong AABB.
    #[expect(dead_code)]
    pub fn model_dbg_normalize_scale(&self) -> f32 {
        self.model
            .as_ref()
            .map_or(1.0, ene_vrm::VrmModel::normalize_scale)
    }

    /// Diagnostic: the merged skeleton joint count after
    /// the multiple-skin merge. Should be the deduplicated total
    /// of every skin's joint list.
    #[expect(dead_code)]
    pub fn model_dbg_merged_skel_joints(&self) -> Option<usize> {
        self.model.as_ref().map(ene_vrm::VrmModel::joint_count)
    }

    /// Diagnostic: camera `view_proj` matrix in column-major
    /// format.
    #[expect(dead_code)]
    pub fn camera_view_proj_dbg(&self) -> [[f32; 4]; 4] {
        self.camera.uniform().map_or([[0.0; 4]; 4], |u| u.view_proj)
    }

    /// Diagnostic: just the view matrix.
    #[expect(dead_code)]
    pub fn camera_view_dbg(&self) -> [[f32; 4]; 4] {
        self.camera.debug_view().to_cols_array_2d()
    }

    /// Diagnostic: just the orthographic projection matrix.
    #[expect(dead_code)]
    pub fn camera_proj_dbg(&self) -> [[f32; 4]; 4] {
        self.camera.debug_proj().to_cols_array_2d()
    }

    /// Diagnostic: `view_proj * model` combined matrix.
    #[expect(dead_code)]
    pub fn model_view_proj_dbg(
        &self,
        character_position: [f32; 3],
        actual_scale: f32,
    ) -> [[f32; 4]; 4] {
        let model = self.model_matrix(character_position.into(), actual_scale);
        let view_proj = self.camera.uniform().map_or([[0.0; 4]; 4], |u| u.view_proj);
        let vp = glam::Mat4::from_cols_array_2d(&view_proj);
        (vp * model).to_cols_array_2d()
    }

    /// Diagnostic: the exact matrix the runtime ships to the GPU.
    #[expect(dead_code)]
    pub fn model_matrix_runtime_dbg(
        &self,
        character_position: [f32; 3],
        actual_scale: f32,
    ) -> [[f32; 4]; 4] {
        self.model_matrix(character_position.into(), actual_scale)
            .to_cols_array_2d()
    }

    /// World-space AABB `(min, max)` of the loaded model after
    /// the per-frame `ModelUniform` is applied.
    #[expect(dead_code)]
    pub fn aabb_world(&self, model_uniform: &ModelUniform) -> Option<(Vec3, Vec3)> {
        let model = self.model.as_ref()?;
        let (lo, hi) = model.aabb();
        let model_mat = glam::Mat4::from_cols_array_2d(&model_uniform.model);
        Some(drag::transformed_aabb_bounds(lo, hi, model_mat))
    }

    /// Build the per-bone [`BoneShapeSpec`] list for the physics
    /// world. Shape category and dimensions come from the
    /// per-vertex skinning weights of the loaded mesh (see
    /// [`crate::character::collider::compute_bone_specs`]).
    /// `actual_scale = auto_fit_scale × model_scale` and the
    /// returned dimensions are in world units.
    #[cfg_attr(target_os = "linux", expect(dead_code))]
    pub fn build_character_bone_specs(&mut self, actual_scale: f32) -> Vec<BoneShapeSpec> {
        let Some(model) = self.model.as_ref() else {
            return Vec::new();
        };
        let specs = crate::character::collider::compute_bone_specs(model, actual_scale);
        self.active_bone_nodes = specs.iter().map(|spec| spec.bone_node).collect();
        specs
    }

    /// Read the current (animated) world-space translation and
    /// rotation of active humanoid bones, in the same order as
    /// [`Self::build_character_bone_specs`]. `update_skin_palette`
    /// inside `update_motion` updates `model.nodes.world_*` every
    /// frame, so reading them here gives the live post-animation
    /// transforms without GPU readback.
    #[cfg_attr(target_os = "linux", expect(dead_code))]
    pub fn current_bone_poses(&self) -> Vec<BonePose> {
        let Some(model) = self.model.as_ref() else {
            return Vec::new();
        };
        let center = Vec3::from(model.center());
        let normalize_scale = model.normalize_scale();
        let mut out = Vec::new();
        for &node_idx in &self.active_bone_nodes {
            let pos_raw = model.nodes.world_positions[node_idx];
            let pos_local = (pos_raw - center) * normalize_scale;
            let rot = model.nodes.world_rotations[node_idx];
            out.push(BonePose {
                translation: pos_local,
                rotation: rot,
            });
        }
        out
    }

    /// Retrieve the humanoid bone name for an active collider index.
    #[cfg_attr(target_os = "linux", expect(dead_code))]
    pub fn get_active_bone_name(&self, idx: usize) -> Option<String> {
        let node_idx = *self.active_bone_nodes.get(idx)?;
        let model = self.model.as_ref()?;
        if let Some((name, _)) = model
            .humanoid
            .iter()
            .find(|(_, entry)| entry.node == node_idx)
        {
            return Some(name.to_string());
        }
        if let Some(spring_bones) = &model.spring_bones {
            for chain in &spring_bones.springs {
                if chain.joints.iter().any(|j| j.node == node_idx) {
                    let category =
                        crate::character::collider::classify_spring_chain(chain.name.as_deref());
                    return Some(category.to_string());
                }
            }
        }
        None
    }
}

/// Compute the world-space position of a humanoid bone:
/// `(bone_raw - center) * normalize_scale * model_scale + character_position`.
fn bone_world_rest_position(model: &VrmModel, bone: &HumanoidBoneEntry) -> Vec3 {
    crate::character::collider::compute_rest_world_positions(model)[bone.node]
}

/// Build a `SpringBoneSimulator` from the loaded model and the
/// parsed `VRMC_springBone` extension. Clones the rest positions
/// and rest local rotations so the simulator can step on its own
/// schedule without the live model data.
fn build_spring_bone_simulator(
    model: &VrmModel,
    props: &SpringBoneProperties,
) -> Option<SpringBoneSimulator> {
    if props.springs.is_empty() {
        return None;
    }
    let world_positions_vec = crate::character::collider::compute_rest_world_positions(model);
    let world_rotations = collect_world_rest_rotations(model);
    let parent_world_rotations = collect_parent_world_rest_rotations(model);
    let world_positions: std::collections::HashMap<usize, Vec3> = (0..world_positions_vec.len())
        .map(|i| (i, world_positions_vec[i]))
        .collect();
    let local_rotations: std::collections::HashMap<usize, Quat> = model
        .nodes
        .rest_local_rotations
        .iter()
        .enumerate()
        .map(|(i, q)| (i, *q))
        .collect();
    let sim = SpringBoneSimulator::new(
        props,
        &world_positions,
        &world_rotations,
        &parent_world_rotations,
        &local_rotations,
    );
    Some(sim)
}

/// Walk the parent chain accumulating rest local rotations to
/// produce per-node world rest rotations (the loader leaves
/// `world_rotations` at identity until `update_skin_palette`
/// fills it).
fn collect_world_rest_rotations(model: &VrmModel) -> std::collections::HashMap<usize, Quat> {
    let n = model.nodes.len();
    let mut out = std::collections::HashMap::with_capacity(n);
    for i in 0..n {
        let mut q = Quat::IDENTITY;
        let mut cur = i as i32;
        // Walk to the root, accumulating rotations.
        while cur >= 0 {
            q = model.nodes.rest_local_rotations[cur as usize] * q;
            cur = model.nodes.parents[cur as usize];
        }
        out.insert(i, q);
    }
    out
}

/// Like `collect_world_rest_rotations` but stops one step short
/// of each node so the simulator has the per-bone parent
/// rotation available.
fn collect_parent_world_rest_rotations(model: &VrmModel) -> std::collections::HashMap<usize, Quat> {
    let n = model.nodes.len();
    let mut out = std::collections::HashMap::with_capacity(n);
    for i in 0..n {
        let parent = model.nodes.parents[i];
        if parent < 0 {
            out.insert(i, Quat::IDENTITY);
        } else {
            out.insert(
                i,
                *out.get(&(parent as usize)).unwrap_or(&Quat::IDENTITY)
                    * model.nodes.rest_local_rotations[parent as usize],
            );
        }
    }
    out
}

fn bone_world_position(
    bone_world_rest_translation: Vec3,
    center: [f32; 3],
    normalize_scale: f32,
    character_position: Vec3,
    model_scale: f32,
) -> Vec3 {
    let center = Vec3::from(center);
    let local = (bone_world_rest_translation - center) * normalize_scale;
    character_position + local * model_scale
}

/// Pick the first available `head` → `chest` → `hips` bone. The
/// caller is expected to fall back to the AABB center
/// (= `character_position` in world space) on `None`.
fn pick_body_center_bone(humanoid: &ene_vrm::HumanoidBoneRegistry) -> Option<&HumanoidBoneEntry> {
    humanoid
        .head()
        .or_else(|| humanoid.chest())
        .or_else(|| humanoid.hips())
}

/// Chest bone's **world** rest Y (sum of the parent chain's
/// `rest_local_positions` Y). Reading `bone.rest.translation[1]`
/// instead would give the local glTF `Node::transform()` —
/// ~0.2 m for a chest deep in the hips → spine → chest chain —
/// and would push the camera target ~7× too low.
fn chest_world_rest_y(model: &VrmModel, bone: &HumanoidBoneEntry) -> f32 {
    bone_world_rest_position(model, bone).y
}

/// Camera target Y. Takes the pre-computed world rest Y (not the
/// animated per-frame value) so VRMA / node-constraint / spring-
/// bone motion cannot make the camera (and therefore the head
/// look-at) jitter.
fn chest_target_y(
    chest_world_rest_y: f32,
    model_center_y: f32,
    normalize_scale: f32,
    model_scale: f32,
) -> f32 {
    (chest_world_rest_y - model_center_y) * normalize_scale * model_scale
}

#[cfg(test)]
mod body_center_tests {
    use super::*;
    use ene_vrm::{BoneRestTransform, HumanoidBoneRegistry};

    fn make_bone(translation: Vec3) -> HumanoidBoneEntry {
        HumanoidBoneEntry {
            node: 0,
            joint: None,
            rest: BoneRestTransform {
                translation,
                rotation: Quat::IDENTITY,
            },
        }
    }

    /// `bone_world_position` applies the loader's
    /// `T(-center) * S(normalize_scale)` and the per-frame
    /// `S(model_scale) * T(character_position)`.
    #[test]
    fn bone_world_position_applies_loader_and_runtime_transforms() {
        let bone = make_bone(Vec3::new(0.0, 2.0, 0.0));
        let center = [0.0, 1.0, 0.0];
        let normalize_scale = 0.75;
        let character_position = Vec3::new(0.5, 0.0, 0.0);
        let model_scale = 1.0;
        // (2.0 - 1.0) * 0.75 = 0.75
        // character_position + 0.75 * 1.0 = (0.5, 0.75, 0.0)
        let world = bone_world_position(
            bone.rest.translation,
            center,
            normalize_scale,
            character_position,
            model_scale,
        );
        assert_eq!(world, Vec3::new(0.5, 0.75, 0.0));
    }

    /// `bone_world_position` must respect `model_scale`: doubling
    /// it doubles the bone's offset from the character position.
    #[test]
    fn bone_world_position_scales_offset_by_model_scale() {
        let bone = make_bone(Vec3::new(0.0, 2.0, 0.0));
        let world = bone_world_position(
            bone.rest.translation,
            [0.0, 1.0, 0.0],
            0.75,
            Vec3::ZERO,
            2.0,
        );
        // 0.75 * 2.0 = 1.5
        assert_eq!(world, Vec3::new(0.0, 1.5, 0.0));
    }

    /// `pick_body_center_bone` must prefer `head` over `chest`
    /// and `chest` over `hips`.
    #[test]
    fn pick_body_center_prefers_head_over_chest_and_hips() {
        let mut reg = HumanoidBoneRegistry::new();
        reg.insert("hips".into(), make_bone(Vec3::new(0.0, 0.5, 0.0)));
        reg.insert("chest".into(), make_bone(Vec3::new(0.0, 1.0, 0.0)));
        reg.insert("head".into(), make_bone(Vec3::new(0.0, 1.5, 0.0)));
        let picked = pick_body_center_bone(&reg).unwrap();
        assert_eq!(picked.rest.translation, Vec3::new(0.0, 1.5, 0.0));
    }

    /// With `head` absent, the chain falls back to `chest`.
    #[test]
    fn pick_body_center_falls_back_to_chest() {
        let mut reg = HumanoidBoneRegistry::new();
        reg.insert("hips".into(), make_bone(Vec3::new(0.0, 0.5, 0.0)));
        reg.insert("chest".into(), make_bone(Vec3::new(0.0, 1.0, 0.0)));
        let picked = pick_body_center_bone(&reg).unwrap();
        assert_eq!(picked.rest.translation, Vec3::new(0.0, 1.0, 0.0));
    }

    /// With `head` and `chest` absent, the chain falls back to
    /// `hips`.
    #[test]
    fn pick_body_center_falls_back_to_hips() {
        let mut reg = HumanoidBoneRegistry::new();
        reg.insert("hips".into(), make_bone(Vec3::new(0.0, 0.5, 0.0)));
        let picked = pick_body_center_bone(&reg).unwrap();
        assert_eq!(picked.rest.translation, Vec3::new(0.0, 0.5, 0.0));
    }

    /// An empty registry must return `None` so the caller can
    /// fall back to the AABB center.
    #[test]
    fn pick_body_center_returns_none_for_empty_registry() {
        let reg = HumanoidBoneRegistry::new();
        assert!(pick_body_center_bone(&reg).is_none());
    }

    /// `body_center_world` with no model loaded must return
    /// `character_position` unchanged (the camera does not stare
    /// at the world origin).
    #[test]
    fn body_center_world_no_model_returns_character_position() {
        let renderer = CharacterRenderer::uninit(std::path::Path::new("."), "missing.vrm");
        let center = renderer.body_center_world(Vec3::new(1.0, 2.0, 3.0), 1.5);
        assert_eq!(center, Vec3::new(1.0, 2.0, 3.0));
    }
}

#[cfg(test)]
mod camera_target_tests {
    //! Pins the chest world rest Y chain walk so the camera
    //! target stays stable when a VRMA motion oscillates the
    //! chest bone.
    use super::*;
    use ene_vrm::{
        BoneRestTransform, ExpressionLayer, HumanoidBoneEntry, HumanoidBoneRegistry,
        NodeConstraintRegistry, NodeHierarchy, Skeleton,
    };
    use glam::Quat;

    /// Build a `VrmModel` with a `nodes[0] → nodes[1] → …`
    /// chain. `chain_y[i]` is the Y offset of node `i` relative
    /// to its parent. The chest bone is registered at the last
    /// node so the world rest Y is the sum of all chain offsets.
    fn model_with_chest_chain(
        chain_y: &[f32],
        chest_y_at_last_node: f32,
        normalize_scale: f32,
    ) -> VrmModel {
        assert!(!chain_y.is_empty(), "chain must have at least a root");

        let mut humanoid = HumanoidBoneRegistry::new();
        humanoid.insert(
            "chest".into(),
            HumanoidBoneEntry {
                node: chain_y.len() - 1,
                joint: None,
                rest: BoneRestTransform {
                    translation: Vec3::new(0.0, chest_y_at_last_node, 0.0),
                    rotation: Quat::IDENTITY,
                },
            },
        );

        let n = chain_y.len();
        let nodes = NodeHierarchy {
            local_rotations: vec![Quat::IDENTITY; n],
            local_positions: (0..n).map(|i| Vec3::new(0.0, chain_y[i], 0.0)).collect(),
            rest_local_rotations: vec![Quat::IDENTITY; n],
            rest_local_positions: (0..n).map(|i| Vec3::new(0.0, chain_y[i], 0.0)).collect(),
            parents: (0..n)
                .map(|i| if i == 0 { -1 } else { (i - 1) as i32 })
                .collect(),
            world_rotations: vec![Quat::IDENTITY; n],
            world_positions: vec![Vec3::ZERO; n],
        };

        VrmModel::new(
            Vec::new(),
            Skeleton {
                inverse_bind: Vec::new(),
                bind_matrices: Vec::new(),
                joint_to_node: Vec::new(),
            },
            [-1.0, -1.0, -1.0],
            [1.0, 1.0, 1.0],
            [0.0, 0.0, 0.0],
            normalize_scale,
            ExpressionLayer::default(),
            humanoid,
            nodes,
            None,
            Vec::new(),
            NodeConstraintRegistry::default(),
            None,
        )
    }

    /// Pin the formula: `(world_rest_y - center_y) *
    /// normalize_scale * model_scale`. A hand-computed
    /// example guards against a typo in the pure helper.
    #[test]
    fn chest_target_y_matches_hand_computed_formula() {
        let got = chest_target_y(1.5, 0.5, 0.75, 2.0);
        // (1.5 - 0.5) * 0.75 * 2.0 = 1.0 * 1.5 = 1.5
        assert_eq!(got, 1.5);
    }

    /// `chest_world_rest_y` must walk the parent chain and
    /// sum the per-link Y offsets. A 3-node chain (hips at
    /// 1.0, spine at +0.2, chest at +0.2) gives a world
    /// chest Y of 1.4. The first-fix code (which read
    /// `chest.rest.translation[1]` directly) would have
    /// returned 0.2 here.
    #[test]
    fn chest_world_rest_y_sums_parent_chain_offsets() {
        let model = model_with_chest_chain(&[1.0, 0.2, 0.2], 0.2, 1.0);
        let chest = model.humanoid.chest().expect("chest registered");
        let world_y = chest_world_rest_y(&model, chest);
        assert!(
            (world_y - 1.4).abs() < 1e-5,
            "expected world rest Y = 1.4 (sum of hips 1.0 + spine 0.2 + chest 0.2), got {world_y}"
        );
    }

    /// `chest_world_rest_y` must sum only Y, not X or Z.
    #[test]
    fn chest_world_rest_y_sums_only_y_axis() {
        let mut humanoid = HumanoidBoneRegistry::new();
        humanoid.insert(
            "chest".into(),
            HumanoidBoneEntry {
                node: 1,
                joint: None,
                rest: BoneRestTransform {
                    translation: Vec3::new(0.0, 0.2, 0.0),
                    rotation: Quat::IDENTITY,
                },
            },
        );
        let nodes = NodeHierarchy {
            local_rotations: vec![Quat::IDENTITY; 2],
            // Parent at (5, 1, -3) — the X and Z must not
            // bleed into the world Y sum.
            local_positions: vec![Vec3::new(5.0, 1.0, -3.0), Vec3::new(0.0, 0.2, 0.0)],
            rest_local_rotations: vec![Quat::IDENTITY; 2],
            rest_local_positions: vec![Vec3::new(5.0, 1.0, -3.0), Vec3::new(0.0, 0.2, 0.0)],
            parents: vec![-1, 0],
            world_rotations: vec![Quat::IDENTITY; 2],
            world_positions: vec![Vec3::ZERO; 2],
        };
        let model = VrmModel::new(
            Vec::new(),
            Skeleton {
                inverse_bind: Vec::new(),
                bind_matrices: Vec::new(),
                joint_to_node: Vec::new(),
            },
            [-1.0, -1.0, -1.0],
            [1.0, 1.0, 1.0],
            [0.0, 0.0, 0.0],
            1.0,
            ExpressionLayer::default(),
            humanoid,
            nodes,
            None,
            Vec::new(),
            NodeConstraintRegistry::default(),
            None,
        );
        let chest = model.humanoid.chest().expect("chest registered");
        let world_y = chest_world_rest_y(&model, chest);
        assert!(
            (world_y - 1.2).abs() < 1e-5,
            "expected world rest Y = 1.2 (parent 1.0 + chest 0.2), got {world_y}"
        );
    }

    /// Camera target Y must come from the chest's **world** rest Y
    /// (sum of parent chain), and must stay stable when a VRMA
    /// motion oscillates the chest bone's animated position.
    #[test]
    fn update_camera_target_is_stable_and_uses_world_rest_y() {
        let mut renderer = CharacterRenderer::uninit(std::path::Path::new("."), "missing.vrm");
        renderer.set_model_for_test(model_with_chest_chain(&[1.0, 0.2], 0.2, 0.5));

        renderer.update_camera_target(1.0);
        let (_eye1, target1, _vh1, _aspect1) = renderer.camera.debug();
        let y_after_first_call = target1[1];

        {
            let model = renderer.model.as_mut().expect("model installed");
            model.nodes.world_positions[1] = Vec3::new(0.0, 5.0, 0.0);
        }
        renderer.update_camera_target(1.0);
        let (_eye2, target2, _vh2, _aspect2) = renderer.camera.debug();
        let y_after_animation = target2[1];

        assert!(
            (y_after_first_call - 0.6).abs() < 1e-5,
            "first call must read the world rest Y (1.2) * 0.5 = 0.6, got {y_after_first_call}"
        );
        assert!(
            (y_after_animation - 0.6).abs() < 1e-5,
            "second call must still read the world rest Y after world_positions[1] is mutated, got {y_after_animation}"
        );
    }

    /// `update_camera_target` with no model must not panic; the
    /// target stays at the orthographic default.
    #[test]
    fn update_camera_target_without_model_keeps_default_target() {
        let mut renderer = CharacterRenderer::uninit(std::path::Path::new("."), "missing.vrm");
        renderer.update_camera_target(1.0);
        let (_eye, target, _vh, _aspect) = renderer.camera.debug();
        assert_eq!(target, [0.0, 0.0, 0.0]);
    }
}
