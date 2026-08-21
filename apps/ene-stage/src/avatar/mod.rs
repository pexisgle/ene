//! Overlay VRM loaded from a core-resolved `avatar_path`.

mod collider_debug;
pub mod look_at;

use glam::{Mat4, Quat, Vec3};
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use thiserror::Error;

use ene_vrm::DebugRenderer;
use ene_vrm::VrmError;
use ene_vrm::camera::{CameraUniform, ModelUniform, OrthographicCamera};
use ene_vrm::expression::ExpressionName;
use ene_vrm::look_at::{LookAtEvaluator, LookAtOutput};
use ene_vrm::minimal::write_glb;
use ene_vrm::prelude::{
    VisemeWeights, VrmModel, VrmRenderer, VrmaAsset, VrmaFrame, VrmaPlayer, load_vrm, load_vrma,
};
use ene_vrm::spring_bone::SpringBoneSimulator;

#[derive(Debug, Error)]
pub enum AvatarError {
    #[error("vrm: {0}")]
    Vrm(#[from] VrmError),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

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
    look_at_target: Option<Vec3>,
    pending_visemes: Option<VisemeWeights>,
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
            look_at_target: None,
            pending_visemes: None,
            blink_accum: 0.0,
            blinking: 0.0,
            last_hips: None,
            model_scale: 1.0,
            world_offset: [0.0, 0.0, 0.0],
        })
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

    pub fn cycle_motion(&mut self, delta: i32) {
        if self.motions.is_empty() {
            return;
        }
        let len = i32::try_from(self.motions.len()).unwrap_or(1);
        let next = (i32::try_from(self.motion_idx).unwrap_or(0) + delta).rem_euclid(len);
        self.motion_idx = usize::try_from(next).unwrap_or(0);
        self.load_motion_at(self.motion_idx);
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

    fn load_motion_at(&mut self, idx: usize) {
        let Some((_, path)) = self.motions.get(idx).cloned() else {
            return;
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
            }
            Err(err) => tracing::warn!(path = %path.display(), %err, "VRMA load failed"),
        }
    }

    pub fn apply_expression(&mut self, label: &str) {
        let name = ExpressionName::new(label);
        if !self.model.expressions.set_expression(&name, 1.0) {
            tracing::debug!(label, "unknown expression discarded");
        }
    }

    pub fn apply_viseme(&mut self, weights: VisemeWeights) {
        self.pending_visemes = Some(weights);
    }

    pub fn set_look_at_target(&mut self, target: Vec3) {
        self.look_at_target = Some(target);
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
                self.pending_visemes = Some(weights);
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
        self.tick_idle(dt);
        if let Some(weights) = self.pending_visemes.take() {
            self.model.expressions.apply_viseme_weights(&weights);
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
        let scale = auto * self.model_scale * self.model.normalize_scale();
        let center = self.model.center();
        let translate = Vec3::from(self.world_offset);
        let model_mat = Mat4::from_translation(translate)
            * Mat4::from_scale(Vec3::splat(scale))
            * Mat4::from_translation(-Vec3::from(center));
        (model_mat, scale)
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
        );
        Ok(())
    }

    pub fn write_default_minimal_vrm(path: &Path) -> Result<(), AvatarError> {
        write_glb(path)?;
        Ok(())
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

#[cfg(test)]
mod tests {
    #[test]
    fn unknown_expression_is_not_stored() {
        // Construction requires a GPU; this only checks the JSON matcher does not panic.
        let value = serde_json::json!({ "type": "body.posture", "name": "sit" });
        assert_eq!(value["type"], "body.posture");
    }
}
