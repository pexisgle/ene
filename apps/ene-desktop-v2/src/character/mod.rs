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
//! - PR4.4: drains due commands from the settings UI's
//!   [`EmotionQueue`](crate::character_state::EmotionQueue) and
//!   pushes the resulting weights into the loaded VRM. After the
//!   active emotion's `hold_secs` elapses the renderer fades the
//!   weight multiplicatively each frame until it drops below
//!   `FADE_FLOOR`, then forgets it.
use std::path::PathBuf;

use ene_vrm::{
    ExpressionName, HumanoidBoneEntry, LookAtBoneOutput, LookAtEvaluator, LookAtOutput,
    LookAtProperties, ModelUniform, OrthographicCamera, VrmModel, VrmRenderer, evaluate_clip,
    load_vrm, load_vrma,
};
use ene_vrm::{VrmaAsset, VrmaPlayer};
use glam::{Mat4, Quat, Vec3};

use crate::character_state::{ActiveEmotion, EmotionQueue, transition_emotions};
use crate::look_at::{LookAtState, compute_world_target, head_world_for};

pub mod collider;
pub mod drag;

pub use collider::{BonePose, BoneShapeSpec};
pub use drag::CharacterDragState;

/// Weight below which an active emotion is considered fully
/// faded and can be discarded.
const FADE_FLOOR: f32 = 0.01;

/// Per-frame fade factor applied to the active emotion's
/// weight once its `hold_secs` elapses. `0.9` means the weight
/// decays to 1 % in ~44 frames (≈ 0.7 s at 60 fps).
const FADE_RATE: f32 = 0.9;

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
    /// PR4.8: latest per-bone output for `"bone"`-type models.
    /// `None` for `"expression"`-type models (which write into
    /// [`VrmModel::expressions_mut`] directly) or before the
    /// first frame. The skinning palette update is the next
    /// pass — when it lands, this field feeds it.
    look_at_bone_output: Option<LookAtBoneOutput>,
    /// PR4.4: currently-applied emotion, used to fade the weight
    /// back to zero after the hold elapses. `None` means the
    /// model is at its neutral state.
    active_emotion: Option<ActiveEmotion>,
    /// PR4.3: drag state. `last_cursor_world_pos.is_some()` means
    /// the user is currently dragging the character.
    pub drag: CharacterDragState,
    /// PR4.16: currently-loaded VRMA asset, or `None` when
    /// no motion has been bound. The runtime calls
    /// [`CharacterRenderer::play_motion`] once on startup
    /// (using the `default_motion` from settings) and may
    /// call it again when the user picks a different
    /// motion from the settings UI.
    vrma: Option<VrmaAsset>,
    /// PR4.16: per-frame VRMA playback state. `time` is
    /// advanced by [`CharacterRenderer::update_motion`].
    vrma_player: VrmaPlayer,
    /// PR4.16: resolved on-disk path of the currently-loaded
    /// VRMA. Tracked alongside `vrma` so the runtime can
    /// log which motion is active and the settings UI
    /// can show a "current motion" label without
    /// re-reading the asset. `None` when no motion is
    /// loaded.
    vrma_path: Option<PathBuf>,
    /// Node indices of bones that have active colliders
    active_bone_nodes: Vec<usize>,
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
            look_at_bone_output: None,
            active_emotion: None,
            drag: CharacterDragState::default(),
            vrma: None,
            vrma_player: VrmaPlayer::default(),
            vrma_path: None,
            active_bone_nodes: Vec::new(),
        }
    }

    /// Test-only: install a fully-built `VrmModel` directly on
    /// the renderer. The normal `init` path goes through
    /// `load_vrm` + a `wgpu` device, which makes pure unit
    /// tests of the per-frame CPU math (e.g. camera target)
    /// awkward. The setter is `pub(crate)` + `#[cfg(test)]`
    /// so it is only visible from sibling test modules.
    #[cfg(test)]
    pub(crate) fn set_model_for_test(&mut self, model: VrmModel) {
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

    /// PR4.16: load a `.vrma` from disk, reset the
    /// playback clock, and store the asset on the
    /// renderer. Safe to call before the model is
    /// loaded (the asset is kept in `self.vrma` and
    /// applied on the first `update_motion` call
    /// that finds a model).
    ///
    /// The runtime calls this on startup with the
    /// `default_motion` resolved from the settings.
    /// Errors are logged and the previous motion is
    /// kept — the model just stays in its rest pose
    /// (which is exactly what the user sees when
    /// `default_motion` is empty or the file is
    /// missing).
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

    /// PR4.16: advance the VrmaPlayer by `dt_secs`,
    /// evaluate the active clip, push expression
    /// weights into the morph layer, and recompute
    /// the skin palette. Returns the new palette
    /// (length `model.joint_count()`) when the GPU
    /// needs a write — the runtime forwards the
    /// returned slice to
    /// [`VrmRenderer::update_skin_palette`]. Returns
    /// `None` when there is no motion, no model, or
    /// the player is paused.
    ///
    /// **Borrow-safety note**: this method borrows
    /// `self` mutably twice (once for the model and
    /// once for the renderer) which would conflict
    /// if both were on `self`. The implementation
    /// only touches the model and avoids the
    /// renderer here; the runtime calls
    /// `VrmRenderer::update_skin_palette` separately
    /// with the returned `Vec<Mat4>`.
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

        // Skin palette. PR4.16: the LookAt bone output (set
        // by `update_look_at` for `"bone"`-type models) is
        // composed on top of the VRMA pose so the head and
        // eyes track the cursor. `None` for `"expression"`-
        // type models, where the LookAt signal routes into
        // morph weights via `apply_emotions` instead.
        let palette = model.update_skin_palette(&frame, self.look_at_bone_output.as_ref());
        if palette.is_empty() {
            None
        } else {
            Some(palette)
        }
    }

    /// PR4.16: forward a new skin palette to the GPU.
    /// The runtime calls this with the
    /// `Vec<Mat4>` returned by
    /// [`CharacterRenderer::update_motion`] and the
    /// per-frame `&wgpu::Queue`. No-op when the
    /// renderer is missing (model failed to load) or
    /// the palette is empty.
    pub fn update_skin_palette_gpu(&self, queue: &wgpu::Queue, palette: &[glam::Mat4]) {
        if let Some(renderer) = self.renderer.as_ref() {
            renderer.update_skin_palette(queue, palette);
        }
    }

    /// PR4.4: drain every due command from `queue`, push the
    /// resulting weights into the loaded VRM, and fade the
    /// active emotion back to zero once its `hold_secs` has
    /// elapsed. Safe to call once per frame from
    /// `Runtime::about_to_wait`; no-op if the model failed to
    /// load.
    ///
    /// `now_secs` is the same clock used by
    /// [`SettingsUi::started_at`](crate::settings_ui::SettingsUi::started_at)
    /// — the runtime passes
    /// `uw.settings_ui.started_at.elapsed().as_secs_f64()` so
    /// queue timestamps and the fade clock stay in lock-step.
    ///
    /// The renderer's `active_emotion` is overwritten by every
    /// new command of a different expression (last write wins);
    /// the same-name case simply refreshes the hold window. This
    /// matches the legacy `bevy_vrm1` "blend stack = single
    /// emotion" behaviour and is sufficient for the AI bridge
    /// and the manual-expression test buttons.
    ///
    /// **Important — weight clearing**: when a new emotion is
    /// drained, the *previous* active emotion's weight is set to
    /// `0.0` in the model's `ExpressionLayer` *before* the new
    /// weight is written. Without this clear, a previous "happy"
    /// weight would survive a click on "neutral" (which is not
    /// a morph target) and keep squinting the eyes forever.
    /// This was the source of the "every expression squints the
    /// eyes" bug reported in the PR4.4 review. The actual
    /// transition computation lives in
    /// [`transition_emotions`](crate::character_state::transition_emotions)
    /// so it can be unit-tested without a live `wgpu` device.
    pub fn apply_emotions(&mut self, queue: &mut EmotionQueue, now_secs: f64) {
        let Some(model) = self.model.as_mut() else {
            return;
        };

        let drained = queue.drain_due(now_secs);
        let (new_active, updates) = transition_emotions(
            &drained,
            self.active_emotion.as_ref(),
            now_secs,
            FADE_RATE,
            FADE_FLOOR,
        );
        for (name, weight) in updates {
            model
                .expressions_mut()
                .set_expression(&ExpressionName::from(name.as_str()), weight);
        }
        // PR4.9: evaluate override semantics after writing
        // the raw per-frame weights. This ensures `happy`
        // with `overrideBlink=block` suppresses blink, and
        // `isBinary` expressions are clamped to 0/1.
        {
            let meta = model.expressions_meta.clone();
            model.expressions_mut().apply_overrides(&meta);
        }
        self.active_emotion = new_active;
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

    /// PR4.2 / PR4.8: update the cursor-driven head-look-at
    /// state.
    ///
    /// `character_position` is the model's pivot in world
    /// space (the legacy `character_state.character_position`).
    /// `model_scale` is the per-frame uniform scale (the legacy
    /// `character_state.model_scale`); both are read by the
    /// runtime from `CharacterState`. The head world position
    /// is derived from the humanoid registry's `head` bone
    /// (when present) so the look-at follows the model's
    /// actual head geometry, with a fallback to
    /// [`head_world_for`] (a 1 m Y offset) for models without
    /// humanoid metadata.
    ///
    /// The smoothed world target is stored in [`LookAtState`]
    /// and returned. PR4.8 also runs the
    /// [`LookAtEvaluator`] on the result and either:
    /// - writes `lookUp` / `lookDown` / `lookLeft` / `lookRight`
    ///   morph weights into the model's
    ///   [`VrmModel::expressions_mut`] for `"expression"`-type
    ///   models (the path the legacy `bevy_vrm1` silently
    ///   no-op'd), or
    /// - stores the per-bone `Quat` deltas in
    ///   [`CharacterRenderer::look_at_bone_output`] for
    ///   `"bone"`-type models. The next skinning palette pass
    ///   consumes this field; for now it sits in memory.
    pub fn update_look_at(
        &mut self,
        cursor_logical: glam::Vec2,
        viewport_size: (u32, u32),
        character_position: Vec3,
        model_scale: f32,
        strength: f32,
        dt_secs: f32,
    ) -> Vec3 {
        // 1. Derive the head world position from the
        //    humanoid registry when possible. PR4.7 ships
        //    the registry; PR4.8 is the first consumer
        //    that actually changes the math based on it
        //    (the previous consumers just read joint
        //    indices). The fallback keeps models without
        //    a humanoid head bone (legacy VRM 0.x) on
        //    the legacy 1 m Y offset.
        let (head_world, head_rest_rotation) = self.head_world_for(character_position, model_scale);

        // 2. Build the look-at properties (model-driven or
        //    spec default).
        let props = self
            .model
            .as_ref()
            .and_then(|m| m.look_at().copied())
            .unwrap_or_default();
        let smoothing = LookAtProperties::DEFAULT_SMOOTHING;

        // 3. Compute the smoothed world target.
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

        // 4. Run the evaluator. For `"expression"`-type
        //    models the result is written straight into
        //    `ExpressionLayer`; for `"bone"`-type models
        //    it is stashed for the next skinning palette
        //    pass to consume.
        //
        //    Clone the expression metadata before mutating
        //    `expressions_mut()` so the borrow checker stays
        //    happy (a typical model has ~30 definitions,
        //    ~60 bytes each — cheap to clone).
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
                    // PR4.9: evaluate override semantics
                    // after writing the gaze weights.
                    // This ensures `happy` with
                    // `overrideLookAt=block` suppresses
                    // the look-at gaze when the model is
                    // expressing happy. The same
                    // `expressions_meta` clone is used
                    // here (the `apply_emotions` call
                    // earlier in the frame writes emotion
                    // weights — overrides apply to both).
                    layer.apply_overrides(&expressions_meta);
                }
                // Clear any stale bone output (the model
                // toggled type at runtime, e.g. via a
                // future settings switch).
                self.look_at_bone_output = None;
            }
            LookAtOutput::Bone(b) => {
                self.look_at_bone_output = Some(b);
            }
        }

        smoothed_target
    }

    /// Compute the world-space head position and rest
    /// rotation. Returns the head's world position and the
    /// bone's rest rotation in `(world_pos, rest_rotation)`
    /// form. The rest rotation is `Quat::IDENTITY` when the
    /// model has no humanoid head bone (the legacy
    /// behaviour, where the head was implicitly aligned
    /// with the world frame).
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

    /// PR-fix: compute the world-space "body center" for camera
    /// targeting. Tries the humanoid `head` bone first, then
    /// `chest`, then `hips`, and falls back to `character_position`
    /// (the AABB center in world space) when the model has no
    /// humanoid bones or no model is loaded.
    ///
    /// The orthographic camera's default target is the world
    /// origin, which is also the model's AABB center after the
    /// `T(-center) * S(normalize_scale)` step. For models whose
    /// AABB is skewed by hair ornaments, weapons, or pedestals,
    /// the AABB center can sit well above the body — making the
    /// character appear pushed to the top of the window. Using a
    /// humanoid bone as the target pins the camera to the body
    /// regardless of the AABB shape.
    ///
    /// Used by [`Self::update_camera_target`].
    #[allow(dead_code)]
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

    /// Update the camera target to perfectly frame the character's AABB
    pub fn update_camera_target(&mut self, model_scale: f32) {
        // PR-fix: the camera target is read by `look_at::compute_world_target`
        // (via the orthographic camera's eye / target) which then drives the
        // head-look-at output every frame. The original implementation read
        // `model.nodes.world_positions[chest.node][1]` — the **animated**
        // chest bone world Y from the previous frame. With an active VRMA
        // motion that oscillated the chest, the camera target oscillated
        // frame-to-frame, the cursor→world projection oscillated with it,
        // and the resulting look-at was visibly choppy.
        //
        // The fix is to read the chest bone's **world rest Y** (computed by
        // walking the parent chain through `nodes.parents` +
        // `nodes.rest_local_positions`). The rest pose is constant for the
        // lifetime of the model, so the camera target is now stable under
        // animation. The first fix pass accidentally read
        // `chest.rest.translation[1]` — the **local** glTF `Node::transform()`
        // (offset from the parent bone) — which made the camera target
        // ~7× too low for a humanoid chest deep in the chain and broke the
        // framing, the look-at, and the drag.
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
    pub fn camera_eye(&self) -> [f32; 3] {
        self.camera.eye()
    }

    /// Returns the current camera target position.
    pub fn camera_target(&self) -> [f32; 3] {
        self.camera.target()
    }

    /// PR5.6: build the per-frame camera uniform for the
    /// debug overlay so its `view_proj` matches what the
    /// main VRM pass used. The orthographic camera's
    /// `uniform()` is infallible in practice, so this
    /// returns `Option` for API symmetry with the debug
    /// pipeline's own fallible future.
    pub fn camera_uniform_dbg(&self) -> Option<ene_vrm::camera::CameraUniform> {
        self.camera.uniform().ok()
    }

    /// PR4.8: latest per-bone output for `"bone"`-type
    /// models. `None` for `"expression"`-type models or
    /// before the first frame. Consumed by
    /// [`Self::update_motion`] (PR4.16) — the value is
    /// forwarded into
    /// [`ene_vrm::VrmModel::update_skin_palette`] every
    /// frame, so the head and eyes track the cursor. The
    /// accessor stays for diagnostics and future
    /// settings-UI work (e.g. an "override LookAt from
    /// expression" toggle that wants to peek at the
    /// current value).
    #[allow(dead_code)] // Diagnostic accessor; the per-frame read happens inside `update_motion`.
    pub fn look_at_bone_output(&self) -> Option<&LookAtBoneOutput> {
        self.look_at_bone_output.as_ref()
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

    /// PR4.2 follow-up diagnostic: (aspect_ratio, eye, target,
    /// viewport_height).
    #[allow(dead_code)] // One-shot diagnostic log only.
    pub fn camera_dbg(&self) -> (f32, [f32; 3], [f32; 3], f32) {
        let (eye, target, viewport_height, aspect) = self.camera.debug();
        (aspect, eye, target, viewport_height)
    }

    /// Compute the per-frame auto-fit scale: the multiplier that
    /// makes the loaded model's normalised AABB fit the current
    /// viewport with `margin` of unused space on every side.
    /// Returns `1.0` if the model is not loaded (e.g. the default
    /// VRM failed to load and the renderer is only clearing).
    ///
    /// The runtime multiplies this by
    /// `settings.character_state.model_scale` so the user's "zoom"
    /// slider still works on top of the fit. At the default
    /// `model_scale = 1.0` the model now hugs the viewport
    /// (with `margin = 0.9` it occupies 90 % of the viewport
    /// height and fits horizontally). Larger `model_scale` values
    /// zoom in past the fit (e.g. `2.0` → 2× the fit size), which
    /// is what the user wants when they want a close-up.
    ///
    /// PR4.18: the vertex buffer is now in raw glTF space; the
    /// model matrix folds `T(-center) * S(normalize_scale) *
    /// T(position)` in at draw time. The fit is computed
    /// against the **normalised** AABB (max extent 1.5) so the
    /// returned scale is the multiplier on the normalised
    /// space — not on the raw space.
    pub fn auto_fit_scale(&self, margin: f32) -> f32 {
        match self.model.as_ref() {
            None => 1.0,
            Some(model) => {
                let (lo, hi) = model.normalized_aabb();
                self.camera.compute_auto_fit_scale(lo, hi, margin)
            }
        }
    }

    /// The full per-character model matrix that folds the
    /// raw-space vertex buffer through the loader's
    /// `T(-center) * S(normalize_scale)` and the per-character
    /// translation / scale.
    ///
    /// `actual_scale` is the (auto-fit) scale to apply on top
    /// of the normalised space. The runtime multiplies this by
    /// the user's `model_scale` setting before calling
    /// [`Self::model_matrix`], so the helper just receives the
    /// already-resolved `actual_scale`.
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

    /// PR4.2 follow-up diagnostic: (depth_width, depth_height).
    #[allow(dead_code)] // One-shot diagnostic log only.
    pub fn depth_size_dbg(&self) -> (u32, u32) {
        self.depth_size
    }

    /// PR5.6: hand the depth attachment to the debug
    /// overlay so it can `LoadOp::Load` the depth the main
    /// VRM pass wrote. Returns `None` before `init` /
    /// `resize` produces a depth texture.
    pub fn depth_view(&self) -> Option<&wgpu::TextureView> {
        self.depth_view.as_ref()
    }

    pub fn model(&self) -> Option<&VrmModel> {
        self.model.as_ref()
    }

    /// PR4.2 follow-up diagnostic: AABB of the loaded vertex data
    /// (min, max). The loader's normalize centres the AABB on
    /// origin so both halves should be symmetric; if not, that's
    /// the bug.
    #[allow(dead_code)] // One-shot diagnostic log only.
    pub fn model_aabb_dbg(&self) -> Option<([f32; 3], [f32; 3])> {
        self.model.as_ref().map(|m| m.aabb())
    }

    /// PR4.19 diagnostic: the loader-captured AABB centre. The
    /// runtime folds `T(-center)` into the model matrix; if the
    /// centre is wildly off (e.g. a hair vertex got included in
    /// the AABB) the model will be shifted out of the viewport.
    #[allow(dead_code)]
    pub fn model_dbg_center(&self) -> [f32; 3] {
        self.model.as_ref().map(|m| m.center()).unwrap_or([0.0; 3])
    }

    /// PR4.19 diagnostic: the loader's `1.5 / max_extent` scale.
    /// `actual_scale × normalize_scale` should be ~1.42 for
    /// Alicia; if the runtime sees a different value, the loader
    /// computed the wrong AABB.
    #[allow(dead_code)]
    pub fn model_dbg_normalize_scale(&self) -> f32 {
        self.model
            .as_ref()
            .map(|m| m.normalize_scale())
            .unwrap_or(1.0)
    }

    /// PR4.19 diagnostic: the merged skeleton joint count after
    /// the multiple-skin merge. Should be the deduplicated total
    /// of every skin's joint list.
    #[allow(dead_code)]
    pub fn model_dbg_merged_skel_joints(&self) -> Option<usize> {
        self.model.as_ref().map(|m| m.joint_count())
    }

    /// PR4.19 diagnostic: the camera's view_proj matrix in
    /// column-major format. Returned as `[[f32; 4]; 4]` so the
    /// runtime can read individual cells for the diagnostic log.
    #[allow(dead_code)]
    pub fn camera_view_proj_dbg(&self) -> [[f32; 4]; 4] {
        self.camera
            .uniform()
            .map(|u| u.view_proj)
            .unwrap_or([[0.0; 4]; 4])
    }

    /// PR4.19 diagnostic: just the view matrix (proj * view is
    /// what's actually uploaded, but the view alone tells us if
    /// `look_at_rh` is producing the expected rotation+translation).
    #[allow(dead_code)]
    pub fn camera_view_dbg(&self) -> [[f32; 4]; 4] {
        self.camera.debug_view().to_cols_array_2d()
    }

    /// PR4.19 diagnostic: just the orthographic projection
    /// matrix. Exposed so the runtime can dump it for the
    /// "5x taller" investigation.
    #[allow(dead_code)]
    pub fn camera_proj_dbg(&self) -> [[f32; 4]; 4] {
        self.camera.debug_proj().to_cols_array_2d()
    }

    /// PR4.19 diagnostic: the full `view_proj * model.model`
    /// matrix. If this dump shows a 5x Y scale, the bug is in
    /// the camera/projection, not in the model matrix.
    #[allow(dead_code)]
    pub fn model_view_proj_dbg(
        &self,
        character_position: [f32; 3],
        actual_scale: f32,
    ) -> [[f32; 4]; 4] {
        let model = self.model_matrix(character_position.into(), actual_scale);
        let view_proj = self
            .camera
            .uniform()
            .map(|u| u.view_proj)
            .unwrap_or([[0.0; 4]; 4]);
        let vp = glam::Mat4::from_cols_array_2d(&view_proj);
        let combined = vp * model;
        combined.to_cols_array_2d()
    }

    /// PR4.19 diagnostic: the **exact** matrix the runtime
    /// produces and ships to the GPU. Captures any divergence
    /// between what we compute here and what the runtime
    /// actually uses (the runtime had been observed to send a
    /// different `char_pos` than the user configured).
    #[allow(dead_code)]
    pub fn model_matrix_runtime_dbg(
        &self,
        character_position: [f32; 3],
        actual_scale: f32,
    ) -> [[f32; 4]; 4] {
        self.model_matrix(character_position.into(), actual_scale)
            .to_cols_array_2d()
    }

    /// PR4.3: world-space AABB `(min, max)` of the loaded model
    /// after the per-frame `ModelUniform` is applied. `None` if no
    /// model is loaded. Reserved for the drag hit-test and future
    /// click-through logic (PR5); the runtime currently computes
    /// the AABB inline so the helper is not yet plumbed.
    #[allow(dead_code)]
    pub fn aabb_world(&self, model_uniform: &ModelUniform) -> Option<(Vec3, Vec3)> {
        let model = self.model.as_ref()?;
        let (lo, hi) = model.aabb();
        let model_mat = glam::Mat4::from_cols_array_2d(&model_uniform.model);
        Some(drag::transformed_aabb_bounds(lo, hi, model_mat))
    }

    /// PR5.2: build the list of `(local_position, radius)` pairs
    /// that the runtime hands to
    /// [`crate::physics::PhysicsWorld::add_character_bone_colliders`].
    /// Each entry corresponds to one humanoid bone. Bones with
    /// an auto-computed radius below [`MIN_BONE_RADIUS`] — almost
    /// always finger segments — are filtered out so the
    /// collider count stays small.
    ///
    /// `actual_scale` is `auto_fit_scale * model_scale`. The
    /// returned **radius** is in world units: leaf bones use a
    /// PR5.4: build the per-bone [`BoneShapeSpec`] list that the
    /// runtime hands to
    /// [`crate::physics::PhysicsWorld::add_character_bone_colliders`].
    /// The shape category (sphere / capsule / capsule_y) and
    /// the dimensions come from the per-vertex skinning
    /// weights of the loaded mesh (see
    /// [`crate::character::collider::compute_bone_specs`]);
    /// finger segments and other bones whose computed radius
    /// falls below [`MIN_BONE_RADIUS`] are dropped.
    ///
    /// `actual_scale = auto_fit_scale × model_scale`. The
    /// returned dimensions are already in world units so the
    /// colliders match the rendered mesh even when the user
    /// has not set `model_scale = 1.0`.
    pub fn build_character_bone_specs(&mut self, actual_scale: f32) -> Vec<BoneShapeSpec> {
        let Some(model) = self.model.as_ref() else {
            return Vec::new();
        };
        let specs = crate::character::collider::compute_bone_specs(model, actual_scale);
        self.active_bone_nodes = specs.iter().map(|spec| spec.bone_node).collect();
        specs
    }

    /// PR5.4: read the **current** (animated) world-space
    /// translation and rotation of active humanoid bones, in
    /// the same order as
    /// [`Self::build_character_bone_specs`]. The animation
    /// system (`update_skin_palette` inside `update_motion`)
    /// updates `model.nodes.world_positions` and
    /// `model.nodes.world_rotations` every frame, so reading
    /// them here gives the live post-animation transforms
    /// without any per-frame GPU readback.
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

/// Compute the world-space position of a humanoid bone. Mirrors
/// the per-frame model matrix's `T(-center) * S(normalize_scale)`
/// re-centring step (see
/// [`CharacterRenderer::model_matrix`]):
///
/// ```text
/// (bone_raw - center) * normalize_scale * model_scale + character_position
/// ```
///
/// `center` and `normalize_scale` come from the loader
/// ([`VrmModel::center`] / [`VrmModel::normalize_scale`]) and
/// are taken as primitives so the helper can be unit-tested
/// without constructing a full `VrmModel`.
fn bone_world_rest_position(model: &VrmModel, bone: &HumanoidBoneEntry) -> Vec3 {
    crate::character::collider::compute_rest_world_positions(model)[bone.node]
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

/// Pick the first available bone from `head` → `chest` → `hips`
/// in the humanoid registry. Returns `None` when none of the
/// three are registered (e.g. legacy VRM 0.x without a humanoid
/// block). The caller is expected to fall back to the AABB
/// center (= `character_position` in world space) in that case.
#[allow(dead_code)]
fn pick_body_center_bone(humanoid: &ene_vrm::HumanoidBoneRegistry) -> Option<&HumanoidBoneEntry> {
    humanoid
        .head()
        .or_else(|| humanoid.chest())
        .or_else(|| humanoid.hips())
}

/// Compute the chest bone's **world** rest Y position by
/// walking the parent chain in the rest pose, accumulating
/// each ancestor's `rest_local_positions` Y. Mirrors the math
/// the loader's `NodeHierarchy::compute_world_transforms`
/// does in `loader.rs` (one frame, but for a single bone's Y
/// axis).
///
/// PR-fix follow-up: the first PR-fix pass read
/// `chest.rest.translation[1]` directly. That field is the
/// **local** glTF `Node::transform()` (the offset from the
/// parent bone), not the world position. For a humanoid
/// chest deep in the chain (hips → spine → chest → …) the
/// local value is just the offset from the parent (~0.2 m)
/// while the world value is the sum of all ancestors
/// (~1.4 m). Using the local value pushed the camera
/// target ~7× lower than the model, framing the head at the
/// very top of the viewport and breaking the look-at
/// (which depends on the camera matrix) and the drag (the
/// user couldn't see the model well enough to click on it).
fn chest_world_rest_y(model: &VrmModel, bone: &HumanoidBoneEntry) -> f32 {
    bone_world_rest_position(model, bone).y
}

/// Compute the Y component of the camera target from the
/// chest bone's **world** rest Y. Pure rest-pose math: the
/// function deliberately takes a pre-computed world rest Y
/// (and not any animated per-frame value) so VRMA /
/// node-constraint / spring-bone motion cannot make the
/// camera (and therefore the head look-at output, which
/// depends on the camera) jitter.
///
/// PR-fix: the previous `update_camera_target` read
/// `model.nodes.world_positions[chest.node][1]` — the
/// previous frame's animated chest world Y — which made the
/// look-at visibly choppy when a motion oscillated the
/// chest. The follow-up replaces that with
/// [`chest_world_rest_y`], which is the **rest** world Y
/// (stable) and not the local rest Y (the first fix
/// accidentally read the local value, which broke the
/// framing).
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

    /// `bone_world_position` must apply the loader's
    /// `T(-center) * S(normalize_scale)` transform followed by
    /// the per-frame `S(model_scale) * T(character_position)`.
    /// Verifies the arithmetic against a hand-computed example.
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

    /// `bone_world_position` must also respect `model_scale`.
    /// Doubling `model_scale` doubles the bone's offset from
    /// the character position, mirroring the model matrix's
    /// `S(actual_scale) * T(position)` step.
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
    /// and `chest` over `hips`. A registry with all three returns
    /// the head, exactly as the camera-target fallback chain
    /// intends.
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

    /// `body_center_world` on an uninitialised renderer (no
    /// model loaded) must return `character_position`
    /// unchanged, so the camera does not stare at the world
    /// origin while the renderer's synthetic fallback is in
    /// place.
    #[test]
    fn body_center_world_no_model_returns_character_position() {
        let renderer = CharacterRenderer::uninit(std::path::Path::new("."), "missing.vrm");
        let center = renderer.body_center_world(Vec3::new(1.0, 2.0, 3.0), 1.5);
        assert_eq!(center, Vec3::new(1.0, 2.0, 3.0));
    }
}

#[cfg(test)]
mod camera_target_tests {
    //! PR-fix regression tests: the camera target must remain
    //! stable when a VRMA motion oscillates the chest bone,
    //! and must use the chest's **world** rest Y (not the
    //! local glTF `Node::transform()`) so a humanoid chest
    //! deep in the chain (hips → spine → chest → …) frames
    //! the model correctly.
    //!
    //! History:
    //! - Original code read `nodes.world_positions[chest.node][1]`
    //!   (animated). Choppy under VRMA.
    //! - First fix read `chest.rest.translation[1]` (local).
    //!   Camera ~7× too low; broke framing + look-at + drag.
    //! - Second fix (this module) walks the parent chain to
    //!   compute the **world** rest Y, then applies the same
    //!   Y arithmetic. The tests below pin the parent-chain
    //!   semantics by building a 2- and a 3-node hierarchy
    //!   where the local and world values differ.
    use super::*;
    use ene_vrm::{
        BoneRestTransform, ExpressionLayer, HumanoidBoneEntry, HumanoidBoneRegistry,
        NodeConstraintRegistry, NodeHierarchy, Skeleton,
    };
    use glam::Quat;

    /// Build a minimal `VrmModel` with a chain of nodes. The
    /// chain is `nodes[0]` (root) → `nodes[1]` (child) →
    /// `nodes[2]` (grandchild) — every `rest_local_position[i]`
    /// is the Y offset of node `i` relative to its parent.
    /// The chest bone is registered at the **last** node so
    /// the world rest Y is the sum of all chain offsets.
    ///
    /// `chain_y` carries the per-link Y offsets; the model
    /// is built with `normalize_scale` (the loader's
    /// `1.5 / max_extent` — drives how much the camera
    /// target Y is shrunk by).
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

    /// `chest_world_rest_y` must not accidentally sum X or Z
    /// — only Y. A 2-link chain with non-zero X / Z offsets
    /// on the parent must still report the world Y as the
    /// pure Y sum.
    #[test]
    fn chest_world_rest_y_sums_only_y_axis() {
        // Custom builder with mixed-axis offsets so any bug
        // in the sum (e.g. summing a Vec3 instead of [1]) is
        // caught immediately.
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

    /// PR-fix: the camera target Y must come from the
    /// chest's **world** rest Y (sum of parent chain), not
    /// the local glTF `Node::transform()`. With a 2-link
    /// chain (parent at rest y=1.0, chest at local y=0.2)
    /// and `center.y=0`, `normalize_scale=0.5`,
    /// `model_scale=1.0`, the world rest Y is 1.2, so the
    /// camera target y must be `1.2 * 0.5 = 0.6`.
    /// Mutating `world_positions` between two
    /// `update_camera_target` calls must leave the target
    /// unchanged (chop test).
    #[test]
    fn update_camera_target_is_stable_and_uses_world_rest_y() {
        let mut renderer = CharacterRenderer::uninit(std::path::Path::new("."), "missing.vrm");
        // 2-link chain: parent (root) at y=1.0, child (chest)
        // at local y=0.2 → world rest Y = 1.2.
        // normalize_scale = 0.5 to match Alicia's loaded value.
        renderer.set_model_for_test(model_with_chest_chain(&[1.0, 0.2], 0.2, 0.5));

        renderer.update_camera_target(1.0);
        let (_eye1, target1, _vh1, _aspect1) = renderer.camera.debug();
        let y_after_first_call = target1[1];

        // Simulate a VRMA frame that has just pushed the
        // chest bone up by 1 m (e.g. the model jumping).
        {
            let model = renderer.model.as_mut().expect("model installed");
            model.nodes.world_positions[1] = Vec3::new(0.0, 5.0, 0.0);
        }
        renderer.update_camera_target(1.0);
        let (_eye2, target2, _vh2, _aspect2) = renderer.camera.debug();
        let y_after_animation = target2[1];

        // 1.2 (world rest Y) * 0.5 (normalize_scale) *
        // 1.0 (model_scale) = 0.6. The first fix returned
        // 0.1 (local only), the original choppy code
        // returned 2.5/5.0 after mutation.
        assert!(
            (y_after_first_call - 0.6).abs() < 1e-5,
            "first call must read the world rest Y (1.2) * 0.5 = 0.6, got {y_after_first_call}"
        );
        assert!(
            (y_after_animation - 0.6).abs() < 1e-5,
            "second call must still read the world rest Y after world_positions[1] is mutated, got {y_after_animation}"
        );
    }

    /// `update_camera_target` on a renderer with no model
    /// must not panic; the camera target stays at the
    /// orthographic default (Vec3::ZERO) so the cleared
    /// window continues to render.
    #[test]
    fn update_camera_target_without_model_keeps_default_target() {
        let mut renderer = CharacterRenderer::uninit(std::path::Path::new("."), "missing.vrm");
        renderer.update_camera_target(1.0);
        let (_eye, target, _vh, _aspect) = renderer.camera.debug();
        assert_eq!(target, [0.0, 0.0, 0.0]);
    }
}
