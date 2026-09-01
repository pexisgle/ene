//! Overlay VRM loaded from a core-resolved `avatar_path`.

mod collider_debug;
pub mod look_at;

use glam::{Mat4, Quat, Vec2, Vec3, Vec4};
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use thiserror::Error;

use ene_vrm::VrmError;
use ene_vrm::camera::{CameraUniform, ModelUniform, OrthographicCamera};
use ene_vrm::expression::ExpressionName;
use ene_vrm::look_at::{LookAtEvaluator, LookAtOutput};
use ene_vrm::minimal::write_glb;
use ene_vrm::prelude::{
    VisemeWeights, VrmModel, VrmRenderer, VrmaAsset, VrmaFrame, VrmaPlayer, load_vrm, load_vrma,
};
use ene_vrm::spring_bone::SpringBoneSimulator;
use ene_vrm::{DebugLine, DebugRenderer};

#[derive(Debug, Error)]
pub enum AvatarError {
    #[error("vrm: {0}")]
    Vrm(#[from] VrmError),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

const EXPRESSION_CUE_HOLD: f32 = 4.0;
const EXPRESSION_CUE_FADE: f32 = 0.3;

/// GPU-resident companion avatar drawn directly into the overlay swapchain.
pub struct CompanionAvatar {
    model: VrmModel,
    renderer: VrmRenderer,
    camera: OrthographicCamera,
    springs: Option<SpringBoneSimulator>,
    vrma: Option<VrmaAsset>,
    player: VrmaPlayer,
    motions: Vec<(String, PathBuf)>,
    motion_idx: usize,
    manual_motion_override: bool,
    look_at_target: Option<Vec3>,
    applied_look_at: Option<Vec3>,
    pending_visemes: Option<VisemeWeights>,
    viseme_open: bool,
    expression_cue: Option<(String, f32, f32)>,
    interaction_feedback: f32,
    blink_accum: f32,
    blinking: f32,
    last_hips: Option<Vec3>,
    pub model_scale: f32,
    pub world_offset: [f32; 3],
}

impl CompanionAvatar {
    pub fn load(
        path: &Path,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        format: wgpu::TextureFormat,
    ) -> Result<Self, AvatarError> {
        let mut model = load_vrm(path, device, queue)?;
        model.nodes.compute_world_transforms();
        let renderer = VrmRenderer::new(device, queue, format, None, &model);
        let springs = model.spring_bones.as_ref().map(|props| {
            let (pos, rot, parent_rot, local_rot) = node_maps(&model.nodes);
            SpringBoneSimulator::new(props, &pos, &rot, &parent_rot, &local_rot)
        });
        Ok(Self {
            model,
            renderer,
            camera: OrthographicCamera::default(),
            springs,
            vrma: None,
            player: VrmaPlayer::default(),
            motions: Vec::new(),
            motion_idx: 0,
            manual_motion_override: false,
            look_at_target: None,
            applied_look_at: None,
            pending_visemes: None,
            viseme_open: false,
            expression_cue: None,
            interaction_feedback: 0.0,
            blink_accum: 0.0,
            blinking: 0.0,
            last_hips: None,
            model_scale: 1.0,
            world_offset: [0.0, 0.0, 0.0],
        })
    }

    /// Human-readable VRM dialect label ("VRM 0.x" / "VRM 1.0") for status
    /// surfaces that show which format the loaded avatar uses.
    #[must_use]
    pub fn format_version_label(&self) -> &str {
        self.model.format_version_label()
    }

    pub fn load_motions(&mut self, dir: &Path) {
        self.motions = discover_motions(dir);
        let idle = self
            .motions
            .iter()
            .position(|(name, _)| name.eq_ignore_ascii_case("idle"))
            .or_else(|| {
                self.motions
                    .iter()
                    .position(|(name, _)| name.to_ascii_uppercase().contains("VRMA_01"))
            });
        if let Some(idx) = idle {
            self.motion_idx = idx;
            self.load_motion_at(idx);
        } else if !self.motions.is_empty() {
            self.load_motion_at(0);
        }
    }

    #[must_use]
    pub fn motion_names(&self) -> Vec<&str> {
        self.motions.iter().map(|(name, _)| name.as_str()).collect()
    }

    #[must_use]
    pub fn current_motion(&self) -> Option<&str> {
        self.motions
            .get(self.motion_idx)
            .map(|(name, _)| name.as_str())
    }

    pub fn select_motion_manually(&mut self, name: &str) -> bool {
        self.manual_motion_override = true;
        self.select_motion_named(name)
    }

    pub fn apply_body_motion(&mut self, name: &str) -> bool {
        if self.manual_motion_override {
            return false;
        }
        self.select_motion_named(name)
    }

    #[must_use]
    pub const fn motion_is_manually_overridden(&self) -> bool {
        self.manual_motion_override
    }

    fn select_motion_named(&mut self, name: &str) -> bool {
        let Some(idx) = self.motions.iter().position(|(motion, _)| motion == name) else {
            return false;
        };
        self.motion_idx = idx;
        self.load_motion_at(idx)
    }

    pub fn reset_motion(&mut self) {
        self.manual_motion_override = false;
        let idle = self
            .motions
            .iter()
            .position(|(name, _)| name.eq_ignore_ascii_case("idle"))
            .or_else(|| {
                self.motions
                    .iter()
                    .position(|(name, _)| name.to_ascii_uppercase().contains("VRMA_01"))
            });
        if let Some(idx) = idle {
            self.motion_idx = idx;
            self.load_motion_at(idx);
        } else {
            self.vrma = None;
            self.player = VrmaPlayer::default();
            self.reset_pose_and_springs();
        }
    }

    pub fn stop_motion(&mut self) {
        self.manual_motion_override = true;
        self.vrma = None;
        self.player = VrmaPlayer::default();
        self.reset_pose_and_springs();
    }

    pub fn cycle_motion(&mut self, delta: i32) {
        if self.motions.is_empty() {
            return;
        }
        let len = i32::try_from(self.motions.len()).unwrap_or(1);
        let next = (i32::try_from(self.motion_idx).unwrap_or(0) + delta).rem_euclid(len);
        self.motion_idx = usize::try_from(next).unwrap_or(0);
        self.manual_motion_override = true;
        let _ = self.load_motion_at(self.motion_idx);
    }

    fn reset_pose_and_springs(&mut self) {
        self.model
            .nodes
            .local_rotations
            .copy_from_slice(&self.model.nodes.rest_local_rotations);
        self.model
            .nodes
            .local_positions
            .copy_from_slice(&self.model.nodes.rest_local_positions);
        self.model.nodes.compute_world_transforms();
        self.springs = self.model.spring_bones.as_ref().map(|props| {
            let (pos, rot, parent_rot, local_rot) = node_maps(&self.model.nodes);
            SpringBoneSimulator::new(props, &pos, &rot, &parent_rot, &local_rot)
        });
        self.last_hips = None;
    }

    fn load_motion_at(&mut self, idx: usize) -> bool {
        let Some((_, path)) = self.motions.get(idx).cloned() else {
            return false;
        };
        match load_vrma(&path) {
            Ok(asset) => {
                tracing::info!(path = %path.display(), "loaded VRMA");
                self.reset_pose_and_springs();
                self.vrma = Some(asset);
                self.player = VrmaPlayer {
                    playing: true,
                    ..VrmaPlayer::default()
                };
                true
            }
            Err(err) => {
                tracing::warn!(path = %path.display(), %err, "VRMA load failed");
                false
            }
        }
    }

    pub fn apply_expression(&mut self, label: &str) {
        self.clear_expression_cue();
        let name = ExpressionName::new(label);
        if !self.model.expressions.set_expression(&name, 1.0) {
            tracing::debug!(label, "unknown expression discarded");
        }
    }

    pub fn apply_expression_cue(&mut self, label: &str) {
        self.apply_expression_cue_weighted(label, 1.0);
    }

    pub fn apply_expression_cue_weighted(&mut self, label: &str, weight: f32) -> bool {
        let weight = if weight.is_finite() {
            weight.clamp(0.0, 1.0)
        } else {
            0.0
        };
        if !self.apply_expression_weighted(label, weight) {
            return false;
        }
        self.expression_cue = Some((label.to_owned(), 0.0, weight));
        true
    }

    pub fn trigger_interaction_feedback(&mut self, strength: f32, expression: &str) {
        let strength = if strength.is_finite() {
            strength.clamp(0.0, 1.0)
        } else {
            0.0
        };
        if strength <= 0.0 {
            return;
        }
        self.interaction_feedback = self.interaction_feedback.max(strength);
        // Prefer the requested expression but fall back to a widely-supported
        // preset so long-press still shows feedback on the bundled Alicia.
        if self.apply_expression_cue_weighted(expression, strength) {
            return;
        }
        if expression != "relaxed" {
            let _ = self.apply_expression_cue_weighted("relaxed", strength);
        }
    }

    fn apply_expression_weighted(&mut self, label: &str, weight: f32) -> bool {
        let name = ExpressionName::new(label);
        if self.model.expressions.set_expression(&name, weight) {
            true
        } else {
            tracing::debug!(label, "unknown expression discarded");
            false
        }
    }

    pub fn tick_expression_cue(&mut self, dt: f32) {
        let Some((label, elapsed, peak)) = self.expression_cue.as_mut() else {
            return;
        };
        *elapsed += dt;
        let remaining = EXPRESSION_CUE_HOLD - *elapsed;
        let label = label.clone();
        let peak = *peak;
        if remaining <= 0.0 {
            let _ = self
                .model
                .expressions
                .set_expression(&ExpressionName::new(label), 0.0);
            self.expression_cue = None;
        } else if remaining < EXPRESSION_CUE_FADE {
            let weight = peak * (remaining / EXPRESSION_CUE_FADE).clamp(0.0, 1.0);
            let _ = self
                .model
                .expressions
                .set_expression(&ExpressionName::new(label), weight);
        }
    }

    pub fn clear_expression_cue(&mut self) {
        if let Some((label, _, _)) = self.expression_cue.take() {
            let _ = self
                .model
                .expressions
                .set_expression(&ExpressionName::new(&label), 0.0);
        }
    }

    pub fn apply_viseme(&mut self, weights: VisemeWeights) {
        if weights.is_silent() {
            if self.pending_visemes.is_none() && !self.viseme_open {
                return;
            }
            self.pending_visemes = Some(VisemeWeights::default());
            return;
        }
        self.pending_visemes = Some(weights);
    }

    pub fn set_look_at_target(&mut self, target: Vec3) {
        self.look_at_target = Some(target);
    }

    pub fn clear_look_at_target(&mut self) {
        self.look_at_target = None;
    }

    pub fn apply_body_event(&mut self, value: &Value) {
        let Some(kind) = value.get("type").and_then(Value::as_str) else {
            return;
        };
        match kind {
            "body.expression" => {
                if let Some(name) = value
                    .get("name")
                    .or_else(|| value.get("label"))
                    .and_then(Value::as_str)
                {
                    self.apply_expression(name);
                }
            }
            "body.motion" => {
                if let Some(name) = value.get("name").and_then(Value::as_str) {
                    if let Some(idx) = self.motions.iter().position(|(n, _)| n == name) {
                        self.motion_idx = idx;
                        self.load_motion_at(idx);
                    } else {
                        tracing::debug!(name, "unknown motion discarded");
                    }
                }
            }
            "body.lookat" | "body.look_at" => {
                let x = value.get("x").and_then(Value::as_f64).unwrap_or(0.0) as f32;
                let y = value.get("y").and_then(Value::as_f64).unwrap_or(0.0) as f32;
                let head = self.head_world();
                self.look_at_target = Some(head + Vec3::new(x, y, 1.8));
            }
            "body.lipsync" | "body.viseme" => {
                let viseme = value
                    .get("viseme")
                    .or_else(|| value.get("name"))
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let weight = value.get("weight").and_then(Value::as_f64).unwrap_or(1.0) as f32;
                let mut weights = VisemeWeights::default();
                match viseme {
                    "aa" => weights.aa = weight,
                    "ih" => weights.ih = weight,
                    "ou" => weights.ou = weight,
                    "ee" => weights.ee = weight,
                    "oh" => weights.oh = weight,
                    other => tracing::debug!(other, "unknown viseme discarded"),
                }
                self.apply_viseme(weights);
            }
            "body.posture" => {}
            other => tracing::debug!(other, "unknown body command discarded"),
        }
    }

    #[must_use]
    pub fn camera(&self) -> &OrthographicCamera {
        &self.camera
    }

    #[must_use]
    pub fn head_world(&self) -> Vec3 {
        self.model
            .humanoid
            .head()
            .and_then(|entry| self.model.nodes.world_positions.get(entry.node).copied())
            .unwrap_or(Vec3::new(0.0, 1.2, 0.0))
    }

    pub fn tick(&mut self, dt: f32) {
        self.interaction_feedback = (self.interaction_feedback - dt * 4.0).max(0.0);
        self.tick_idle(dt);
        if let Some(weights) = self.pending_visemes.take() {
            self.model.expressions.apply_viseme_weights(&weights);
            self.viseme_open = !weights.is_silent();
        }
        let frame = self.sample_motion(dt);
        let look_at = self.eval_look_at();
        let bone = match look_at.as_ref() {
            Some(LookAtOutput::Bone(bone)) => Some(bone),
            _ => None,
        };
        if let Some(LookAtOutput::Expression(expr)) = look_at {
            for (name, weight) in [
                ("lookUp", expr.look_up),
                ("lookDown", expr.look_down),
                ("lookLeft", expr.look_left),
                ("lookRight", expr.look_right),
            ] {
                let _ = self
                    .model
                    .expressions
                    .set_expression(&ExpressionName::new(name), weight);
            }
        }
        for (name, weight) in &frame.expression_weights {
            let _ = self
                .model
                .expressions
                .set_expression(&ExpressionName::new(name.as_str()), *weight);
        }
        let hips = frame.hips_translation;
        self.model.update_skin_palette(&frame, bone);
        self.last_hips = hips;
        self.step_springs(dt, hips);
        self.applied_look_at = self.look_at_target;
    }

    fn tick_idle(&mut self, dt: f32) {
        self.blink_accum += dt;
        if self.blinking > 0.0 {
            self.blinking = (self.blinking - dt).max(0.0);
            let weight = if self.blinking > 0.06 {
                1.0
            } else {
                self.blinking / 0.06
            };
            let _ = self
                .model
                .expressions
                .set_expression(&ExpressionName::new("blink"), weight);
            return;
        }
        let _ = self
            .model
            .expressions
            .set_expression(&ExpressionName::new("blink"), 0.0);
        if self.blink_accum > 3.2 {
            self.blink_accum = 0.0;
            self.blinking = 0.14;
        }
    }

    fn sample_motion(&mut self, dt: f32) -> VrmaFrame {
        let Some(asset) = self.vrma.as_ref() else {
            return empty_frame();
        };
        let Some(clip) = asset.clips.first() else {
            return empty_frame();
        };
        let duration = clip.duration.max(0.001);
        self.player.advance(dt, duration);
        let mut frame = self.model.evaluate_vrma(asset, clip, self.player.time);
        if let Some(hips) = frame.hips_translation.as_mut() {
            let rest_xz = self
                .model
                .humanoid
                .hips()
                .and_then(|entry| {
                    self.model
                        .nodes
                        .rest_local_positions
                        .get(entry.node)
                        .copied()
                })
                .unwrap_or(Vec3::ZERO);
            hips.x = rest_xz.x;
            hips.z = rest_xz.z;
        }
        frame
    }

    fn eval_look_at(&self) -> Option<LookAtOutput> {
        let target = self.look_at_target?;
        let props = self.model.look_at.unwrap_or_default();
        let evaluator = LookAtEvaluator::new(&props);
        let head = self.head_world();
        let rest = self
            .model
            .humanoid
            .head()
            .and_then(|entry| {
                self.model
                    .nodes
                    .rest_local_rotations
                    .get(entry.node)
                    .copied()
            })
            .unwrap_or(Quat::IDENTITY);
        Some(evaluator.evaluate(head, target, rest))
    }

    fn step_springs(&mut self, dt: f32, hips: Option<Vec3>) {
        let Some(props) = self.model.spring_bones.clone() else {
            return;
        };
        let Some(sim) = self.springs.as_mut() else {
            return;
        };
        let (pos, rot, parent_rot, _) = node_maps(&self.model.nodes);
        let updates = sim.step(dt, &props, &pos, &rot, &parent_rot, &pos, &rot);
        for (node, local) in updates {
            if let Some(slot) = self.model.nodes.local_rotations.get_mut(node) {
                *slot = local;
            }
        }
        let _ = self.model.rebuild_skin_palette(hips);
    }

    fn overlay_model_transform(&self) -> (Mat4, f32) {
        let (aabb_min, aabb_max) = self.model.normalized_aabb();
        let auto = self.camera.compute_auto_fit_scale(aabb_min, aabb_max, 0.9);
        let feedback_scale = 1.0 + self.interaction_feedback * 0.06;
        let scale = auto * self.model_scale * feedback_scale * self.model.normalize_scale();
        let center = self.model.center();
        let translate = Vec3::from(self.world_offset);
        let model_mat = Mat4::from_translation(translate)
            * Mat4::from_scale(Vec3::splat(scale))
            * Mat4::from_translation(-Vec3::from(center));
        (model_mat, scale)
    }

    /// World-space AABB of the rendered body for hit-testing.
    ///
    /// The render matrix subtracts the model center before scaling; applying
    /// it directly would double-subtract the center. Scaling the normalized
    /// AABB alone reproduces the same world extent because the normalized
    /// space is already centered.
    #[must_use]
    pub fn world_aabb(&self) -> (Vec3, Vec3) {
        let (nmin, nmax) = self.model.normalized_aabb();
        let auto = self.camera.compute_auto_fit_scale(nmin, nmax, 0.9);
        let feedback_scale = 1.0 + self.interaction_feedback * 0.06;
        let scale = auto * self.model_scale * feedback_scale * self.model.normalize_scale();
        let offset = Vec3::from(self.world_offset);
        (
            Vec3::from(nmin) * scale + offset,
            Vec3::from(nmax) * scale + offset,
        )
    }

    #[must_use]
    pub fn overlay_bone_world(&self, bone: &str) -> Option<Vec3> {
        let entry = self.model.humanoid.by_name(bone)?;
        let pos = *self.model.nodes.world_positions.get(entry.node)?;
        let (mat, _) = self.overlay_model_transform();
        Some((mat * pos.extend(1.0)).truncate())
    }

    /// Coarse CPU collider AABB for one body part. `None` when the bone is missing.
    #[must_use]
    pub fn part_world_aabb(&self, part: crate::scene::AvatarPart) -> Option<(Vec3, Vec3)> {
        match part {
            crate::scene::AvatarPart::Body => Some(self.world_aabb()),
            crate::scene::AvatarPart::Head => self.sphere_aabb(&["head", "neck"], 0.12),
            crate::scene::AvatarPart::Torso => {
                self.sphere_aabb(&["chest", "upperchest", "spine"], 0.16)
            }
            crate::scene::AvatarPart::LeftHand => {
                self.sphere_aabb(&["lefthand", "leftmiddleproximal"], 0.07)
            }
            crate::scene::AvatarPart::RightHand => {
                self.sphere_aabb(&["righthand", "rightmiddleproximal"], 0.07)
            }
        }
    }

    fn sphere_aabb(&self, bones: &[&str], radius: f32) -> Option<(Vec3, Vec3)> {
        let center = bones
            .iter()
            .find_map(|name| self.overlay_bone_world(name))?;
        Some((center - Vec3::splat(radius), center + Vec3::splat(radius)))
    }

    #[must_use]
    pub fn needs_redraw(&self) -> bool {
        self.vrma.is_some()
            || self.pending_visemes.is_some()
            || self.viseme_open
            || look_at_is_dirty(self.look_at_target, self.applied_look_at)
            || self.blinking > 0.0
            || self.expression_cue.is_some()
            || self.interaction_feedback > 0.0
    }

    pub(crate) fn push_part_collider_wires(&self, debug: &mut DebugRenderer) {
        const PARTS: [crate::scene::AvatarPart; 4] = [
            crate::scene::AvatarPart::Head,
            crate::scene::AvatarPart::Torso,
            crate::scene::AvatarPart::LeftHand,
            crate::scene::AvatarPart::RightHand,
        ];
        let colors = [
            Vec4::new(1.0, 0.45, 0.2, 1.0),
            Vec4::new(0.3, 0.9, 0.4, 1.0),
            Vec4::new(0.95, 0.85, 0.2, 1.0),
            Vec4::new(0.95, 0.85, 0.2, 1.0),
        ];
        for (part, color) in PARTS.iter().zip(colors) {
            let Some((min, max)) = self.part_world_aabb(*part) else {
                continue;
            };
            let corners = aabb_corners(min, max);
            const EDGES: [(usize, usize); 12] = [
                (0, 1),
                (0, 2),
                (0, 4),
                (1, 3),
                (1, 5),
                (2, 3),
                (2, 6),
                (3, 7),
                (4, 5),
                (4, 6),
                (5, 7),
                (6, 7),
            ];
            for (a, b) in EDGES {
                debug.push_line(DebugLine {
                    a: corners[a],
                    b: corners[b],
                    color,
                });
            }
        }
    }

    /// Clamps a requested translation so the rendered AABB stays inside the
    /// camera viewport, accounting for the current model scale and aspect.
    #[must_use]
    pub fn fit_world_offset(&self, desired: [f32; 3], viewport: (u32, u32)) -> [f32; 3] {
        #[expect(
            clippy::cast_precision_loss,
            reason = "swapchain pixels are well inside f32"
        )]
        let aspect = (viewport.0.max(1) as f32 / viewport.1.max(1) as f32).max(0.0001);
        let mut camera = self.camera.clone();
        camera.set_aspect(aspect);
        let view_projection = camera.debug_proj() * camera.debug_view();
        let (nmin, nmax) = self.model.normalized_aabb();
        let auto = camera.compute_auto_fit_scale(nmin, nmax, 0.9);
        let scale = auto * self.model_scale * self.model.normalize_scale();
        let corners = aabb_corners(Vec3::from(nmin) * scale, Vec3::from(nmax) * scale);
        let projected = corners.map(|corner| project_ndc(view_projection, corner));
        let mut min = Vec2::splat(f32::INFINITY);
        let mut max = Vec2::splat(f32::NEG_INFINITY);
        for point in projected {
            min = min.min(point);
            max = max.max(point);
        }

        const VIEWPORT_MARGIN: f32 = 0.98;
        let origin = project_ndc(view_projection, Vec3::ZERO);
        let requested = Vec3::from(desired);
        let requested_translation = project_ndc(view_projection, requested) - origin;
        let target_translation = Vec2::new(
            clamp_ndc_translation(
                requested_translation.x,
                -VIEWPORT_MARGIN - min.x,
                VIEWPORT_MARGIN - max.x,
            ),
            clamp_ndc_translation(
                requested_translation.y,
                -VIEWPORT_MARGIN - min.y,
                VIEWPORT_MARGIN - max.y,
            ),
        );
        let correction = target_translation - requested_translation;
        let x_basis = project_ndc(view_projection, Vec3::X) - origin;
        let y_basis = project_ndc(view_projection, Vec3::Y) - origin;
        let determinant = x_basis.x * y_basis.y - y_basis.x * x_basis.y;
        let (delta_x, delta_y) = if determinant.abs() > 0.0001 {
            (
                (correction.x * y_basis.y - y_basis.x * correction.y) / determinant,
                (x_basis.x * correction.y - correction.x * x_basis.y) / determinant,
            )
        } else {
            (correction.x, correction.y)
        };
        [requested.x + delta_x, requested.y + delta_y, requested.z]
    }

    pub(crate) fn push_interaction_outline(&self, debug: &mut DebugRenderer) {
        let (min, max) = self.world_aabb();
        let corners = aabb_corners(min, max);
        const EDGES: [(usize, usize); 12] = [
            (0, 1),
            (0, 2),
            (0, 4),
            (1, 3),
            (1, 5),
            (2, 3),
            (2, 6),
            (3, 7),
            (4, 5),
            (4, 6),
            (5, 7),
            (6, 7),
        ];
        let color = Vec4::new(0.2, 0.85, 1.0, 1.0);
        for (a, b) in EDGES {
            debug.push_line(DebugLine {
                a: corners[a],
                b: corners[b],
                color,
            });
        }
    }

    pub(crate) fn push_spring_collider_wires(&self, debug: &mut DebugRenderer) {
        let Some(props) = self.model.spring_bones.as_ref() else {
            return;
        };
        let (model, scale) = self.overlay_model_transform();
        for line in
            collider_debug::collider_debug_lines(&props.colliders, &self.model.nodes, model, scale)
        {
            debug.push_line(line);
        }
    }

    #[must_use]
    pub(crate) fn debug_camera_uniform(&self) -> Option<CameraUniform> {
        self.camera.uniform().ok()
    }

    pub fn render_to_texture(
        &mut self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        depth_view: &wgpu::TextureView,
        width: u32,
        height: u32,
        clear: bool,
    ) -> Result<(), AvatarError> {
        #[expect(
            clippy::cast_precision_loss,
            reason = "swapchain pixels are well inside f32"
        )]
        let aspect = (width.max(1) as f32 / height.max(1) as f32).max(0.0001);
        self.camera.set_aspect(aspect);
        let (model_mat, _) = self.overlay_model_transform();
        let uniform = ModelUniform::from_mat4(model_mat);
        let palette = self.model.rebuild_skin_palette(self.last_hips);
        self.renderer.update_skin_palette(queue, palette);
        self.renderer.render(
            queue,
            encoder,
            view,
            depth_view,
            &self.model,
            &self.camera,
            &uniform,
            true,
            clear,
        );
        Ok(())
    }

    pub fn write_default_minimal_vrm(path: &Path) -> Result<(), AvatarError> {
        write_glb(path)?;
        Ok(())
    }
}

fn aabb_corners(min: Vec3, max: Vec3) -> [Vec3; 8] {
    [
        Vec3::new(min.x, min.y, min.z),
        Vec3::new(min.x, min.y, max.z),
        Vec3::new(min.x, max.y, min.z),
        Vec3::new(min.x, max.y, max.z),
        Vec3::new(max.x, min.y, min.z),
        Vec3::new(max.x, min.y, max.z),
        Vec3::new(max.x, max.y, min.z),
        Vec3::new(max.x, max.y, max.z),
    ]
}

fn project_ndc(view_projection: Mat4, point: Vec3) -> Vec2 {
    let clip = view_projection * point.extend(1.0);
    if clip.w.abs() <= f32::EPSILON {
        Vec2::ZERO
    } else {
        Vec2::new(clip.x / clip.w, clip.y / clip.w)
    }
}

fn clamp_ndc_translation(value: f32, lower: f32, upper: f32) -> f32 {
    if lower <= upper {
        value.clamp(lower, upper)
    } else {
        f32::midpoint(lower, upper)
    }
}

fn empty_frame() -> VrmaFrame {
    VrmaFrame {
        bone_rotations: HashMap::new(),
        hips_translation: None,
        expression_weights: HashMap::new(),
        look_at_yaw_pitch: None,
    }
}

type NodeMaps = (
    HashMap<usize, Vec3>,
    HashMap<usize, Quat>,
    HashMap<usize, Quat>,
    HashMap<usize, Quat>,
);

fn node_maps(nodes: &ene_vrm::NodeHierarchy) -> NodeMaps {
    let mut pos = HashMap::new();
    let mut rot = HashMap::new();
    let mut parent_rot = HashMap::new();
    let mut local_rot = HashMap::new();
    for i in 0..nodes.len() {
        pos.insert(i, nodes.world_positions[i]);
        rot.insert(i, nodes.world_rotations[i]);
        local_rot.insert(i, nodes.rest_local_rotations[i]);
        let parent = nodes.parents[i];
        let parent_q = if parent < 0 {
            Quat::IDENTITY
        } else {
            nodes.world_rotations[parent as usize]
        };
        parent_rot.insert(i, parent_q);
    }
    (pos, rot, parent_rot, local_rot)
}

fn discover_motions(dir: &Path) -> Vec<(String, PathBuf)> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let is_vrma = path
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| ext.eq_ignore_ascii_case("vrma"));
        if !is_vrma {
            continue;
        }
        let name = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or("motion")
            .to_owned();
        out.push((name, path));
    }
    out.sort_by(|left, right| left.0.cmp(&right.0));
    out
}

/// True when the look-at target has moved enough to justify another GPU frame.
#[must_use]
pub fn look_at_is_dirty(target: Option<Vec3>, applied: Option<Vec3>) -> bool {
    match (target, applied) {
        (Some(next), Some(was)) => next.distance_squared(was) > 1e-6,
        (Some(_), None) => true,
        (None, _) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn try_test_device() -> Option<(wgpu::Device, wgpu::Queue)> {
        pollster::block_on(async {
            let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
                backends: wgpu::Backends::PRIMARY,
                backend_options: wgpu::BackendOptions::default(),
                flags: wgpu::InstanceFlags::default(),
                display: None,
                memory_budget_thresholds: wgpu::MemoryBudgetThresholds::default(),
            });
            let adapter = instance
                .request_adapter(&wgpu::RequestAdapterOptions {
                    power_preference: wgpu::PowerPreference::LowPower,
                    compatible_surface: None,
                    force_fallback_adapter: true,
                })
                .await
                .ok()?;
            adapter
                .request_device(&wgpu::DeviceDescriptor {
                    label: Some("ene-stage-avatar-test"),
                    required_features: wgpu::Features::empty(),
                    required_limits: {
                        let mut limits =
                            wgpu::Limits::downlevel_defaults().using_resolution(adapter.limits());
                        limits.max_bind_groups = adapter.limits().max_bind_groups;
                        limits
                    },
                    memory_hints: wgpu::MemoryHints::default(),
                    experimental_features: wgpu::ExperimentalFeatures::disabled(),
                    trace: wgpu::Trace::Off,
                })
                .await
                .ok()
        })
    }

    #[test]
    fn unknown_expression_is_not_stored() {
        let value = serde_json::json!({ "type": "body.posture", "name": "sit" });
        assert_eq!(value["type"], "body.posture");
    }

    #[test]
    fn look_at_is_dirty_only_when_the_target_moves() {
        assert!(!look_at_is_dirty(None, None));
        assert!(!look_at_is_dirty(None, Some(Vec3::ZERO)));
        assert!(look_at_is_dirty(Some(Vec3::ZERO), None));
        assert!(!look_at_is_dirty(Some(Vec3::ZERO), Some(Vec3::ZERO)));
        assert!(look_at_is_dirty(
            Some(Vec3::new(1.0, 0.0, 0.0)),
            Some(Vec3::ZERO)
        ));
        assert!(!look_at_is_dirty(
            Some(Vec3::new(1e-5, 0.0, 0.0)),
            Some(Vec3::ZERO)
        ));
    }

    #[test]
    fn silent_viseme_does_not_keep_the_overlay_dirty() {
        let Some((device, queue)) = try_test_device() else {
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("minimal.vrm");
        CompanionAvatar::write_default_minimal_vrm(&path).unwrap();
        let mut avatar =
            CompanionAvatar::load(&path, &device, &queue, wgpu::TextureFormat::Bgra8UnormSrgb)
                .expect("minimal VRM loads");
        avatar.apply_viseme(VisemeWeights::default());
        assert!(
            !avatar.needs_redraw(),
            "closed mouth must not force a GPU frame"
        );

        avatar.apply_viseme(VisemeWeights {
            aa: 0.8,
            ..VisemeWeights::default()
        });
        assert!(
            avatar.needs_redraw(),
            "speech visemes must dirty the overlay"
        );
        avatar.tick(0.0);
        assert!(
            avatar.needs_redraw(),
            "an open mouth must keep the 16ms wake"
        );

        avatar.apply_viseme(VisemeWeights::default());
        assert!(
            avatar.needs_redraw(),
            "the closing frame must still apply zero weights"
        );
        avatar.tick(0.0);
        assert!(
            !avatar.needs_redraw(),
            "after the mouth closes, silence is not dirty"
        );
        avatar.apply_viseme(VisemeWeights::default());
        assert!(
            !avatar.needs_redraw(),
            "repeated silence must not re-dirty the overlay"
        );
    }

    #[test]
    fn default_minimal_vrm_writes_parseable_glb() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("minimal.vrm");
        CompanionAvatar::write_default_minimal_vrm(&path).unwrap();
        if let Ok(dest) = std::env::var("ENE_WRITE_MINIMAL_VRM") {
            let dest = std::path::Path::new(&dest);
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::fs::copy(&path, dest).unwrap();
        }
        let bytes = std::fs::read(&path).unwrap();
        assert!(bytes.starts_with(b"glTF"));
        assert!(
            bytes.windows(8).any(|window| window == b"VRMC_vrm"),
            "minimal fixture must declare VRMC_vrm"
        );
    }

    #[test]
    fn companion_avatar_loads_the_minimal_fixture() {
        let Some((device, queue)) = try_test_device() else {
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("minimal.vrm");
        CompanionAvatar::write_default_minimal_vrm(&path).unwrap();
        let avatar =
            CompanionAvatar::load(&path, &device, &queue, wgpu::TextureFormat::Bgra8UnormSrgb)
                .expect("minimal VRM loads");
        assert!(avatar.format_version_label().contains("VRM"));
        assert!(avatar.motion_names().is_empty());
    }

    #[test]
    fn explicit_expression_cancels_pending_auto_cue() {
        let Some((device, queue)) = try_test_device() else {
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("minimal.vrm");
        CompanionAvatar::write_default_minimal_vrm(&path).unwrap();
        // The minimal fixture defines no expressions; inject a synthetic
        // "happy" definition so cue state transitions are observable without
        // depending on the license-restricted Alicia asset.
        let mut avatar =
            CompanionAvatar::load(&path, &device, &queue, wgpu::TextureFormat::Rgba8UnormSrgb)
                .unwrap();
        avatar
            .model
            .expressions
            .weights
            .insert(ExpressionName::new("happy"), 0.0);

        avatar.apply_expression_cue("happy");
        assert!(avatar.expression_cue.is_some(), "cue should start");

        avatar.tick_expression_cue(0.1);
        avatar.apply_expression("happy");
        assert!(
            avatar.expression_cue.is_none(),
            "explicit expression must clear the auto cue"
        );

        avatar.tick_expression_cue(f32::MAX);
        assert!(
            avatar.expression_cue.is_none(),
            "ticking after explicit override must not resurrect the cue"
        );
    }
}
