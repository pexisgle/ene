//! VRM companion avatar rendering for the stage overlay.

pub mod look_at;

use std::path::Path;
use std::sync::mpsc;

use egui::{ColorImage, TextureHandle, TextureOptions};
use ene_vrm::camera::{ModelUniform, OrthographicCamera};
use ene_vrm::expression::ExpressionName;
use ene_vrm::expression_override::apply_overrides;
use ene_vrm::look_at::{LookAtBoneOutput, LookAtEvaluator, LookAtOutput};
use ene_vrm::minimal::write_glb;
use ene_vrm::model::VrmModel;
use ene_vrm::prelude::{load_vrm, VrmRenderer};
use ene_vrm::viseme::VisemeWeights;
use ene_vrm::{VrmError, VrmResult};
use glam::{Mat4, Vec3};
use thiserror::Error;
use wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;

use ene_vrm::animation::VrmaFrame;

/// Errors from avatar load/render paths.
#[derive(Debug, Error)]
pub enum AvatarError {
    #[error("VRM: {0}")]
    Vrm(#[from] VrmError),
    #[error("no avatar loaded")]
    NotLoaded,
    #[error("texture readback failed")]
    Readback,
    #[error("GPU buffer map failed")]
    Map,
}

/// Metadata for painting the avatar into an egui panel via [`egui::Image`].
#[derive(Clone, Copy, Debug)]
pub struct VrmPaintInfo {
    pub texture_id: egui::TextureId,
    pub size: [f32; 2],
}

/// Offscreen VRM renderer that periodically copies frames into an egui texture.
pub struct VrmPane {
    avatar: CompanionAvatar,
    color_texture: Option<wgpu::Texture>,
    depth_texture: Option<wgpu::Texture>,
    readback_buffer: Option<wgpu::Buffer>,
    width: u32,
    height: u32,
    frame_counter: u32,
    update_every_n: u32,
    egui_texture: Option<TextureHandle>,
    surface_format: wgpu::TextureFormat,
}

impl VrmPane {
    /// How often (in UI frames) to copy the GPU render into the egui texture.
    pub const DEFAULT_UPDATE_EVERY_N: u32 = 2;

    #[must_use]
    pub fn avatar(&self) -> &CompanionAvatar {
        &self.avatar
    }

    #[must_use]
    pub fn avatar_mut(&mut self) -> &mut CompanionAvatar {
        &mut self.avatar
    }

    /// Load a VRM from disk and prepare offscreen targets sized to `width` x `height`.
    pub fn load(
        path: &Path,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        surface_format: wgpu::TextureFormat,
        width: u32,
        height: u32,
    ) -> Result<Self, AvatarError> {
        let avatar = CompanionAvatar::load(path, device, queue, surface_format)?;
        let mut pane = Self {
            avatar,
            color_texture: None,
            depth_texture: None,
            readback_buffer: None,
            width: width.max(1),
            height: height.max(1),
            frame_counter: 0,
            update_every_n: Self::DEFAULT_UPDATE_EVERY_N,
            egui_texture: None,
            surface_format,
        };
        pane.ensure_targets(device);
        Ok(pane)
    }

    /// Resize offscreen targets when the UI panel changes size.
    pub fn resize(&mut self, device: &wgpu::Device, width: u32, height: u32) {
        let width = width.max(1);
        let height = height.max(1);
        if self.width == width && self.height == height {
            return;
        }
        self.width = width;
        self.height = height;
        self.color_texture = None;
        self.depth_texture = None;
        self.readback_buffer = None;
        self.ensure_targets(device);
    }

    /// Advance idle animation and optionally refresh the egui texture.
    pub fn tick_ui_frame(
        &mut self,
        ctx: &egui::Context,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        dt: f32,
    ) -> Result<Option<VrmPaintInfo>, AvatarError> {
        self.avatar.tick(dt);
        self.frame_counter = self.frame_counter.wrapping_add(1);
        if !self.frame_counter.is_multiple_of(self.update_every_n) {
            return Ok(self.paint_info());
        }
        self.render_and_upload(ctx, device, queue)?;
        Ok(self.paint_info())
    }

    /// Last registered egui paint metadata, if any.
    #[must_use]
    pub fn paint_info(&self) -> Option<VrmPaintInfo> {
        self.egui_texture.as_ref().map(|handle| VrmPaintInfo {
            texture_id: handle.id(),
            size: [self.width as f32, self.height as f32],
        })
    }

    fn ensure_targets(&mut self, device: &wgpu::Device) {
        if self.color_texture.is_some() {
            return;
        }
        let color = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("ene-stage.vrm.color"),
            size: wgpu::Extent3d {
                width: self.width,
                height: self.height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: self.surface_format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let depth = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("ene-stage.vrm.depth"),
            size: wgpu::Extent3d {
                width: self.width,
                height: self.height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth32Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let padded_bytes_per_row = padded_bytes_per_row(self.width);
        let buffer_size = u64::from(padded_bytes_per_row) * u64::from(self.height);
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("ene-stage.vrm.readback"),
            size: buffer_size,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        self.color_texture = Some(color);
        self.depth_texture = Some(depth);
        self.readback_buffer = Some(readback);
    }

    fn render_and_upload(
        &mut self,
        ctx: &egui::Context,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) -> Result<(), AvatarError> {
        let color = self
            .color_texture
            .as_ref()
            .ok_or(AvatarError::NotLoaded)?;
        let depth = self
            .depth_texture
            .as_ref()
            .ok_or(AvatarError::NotLoaded)?;
        let readback = self
            .readback_buffer
            .as_ref()
            .ok_or(AvatarError::NotLoaded)?;

        let color_view = color.create_view(&wgpu::TextureViewDescriptor::default());
        let depth_view = depth.create_view(&wgpu::TextureViewDescriptor::default());

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("ene-stage.vrm.encoder"),
        });
        self.avatar.render_to_texture(
            queue,
            &mut encoder,
            &color_view,
            &depth_view,
            self.width,
            self.height,
        )?;

        let padded_bytes_per_row = padded_bytes_per_row(self.width);
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: color,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded_bytes_per_row),
                    rows_per_image: Some(self.height),
                },
            },
            wgpu::Extent3d {
                width: self.width,
                height: self.height,
                depth_or_array_layers: 1,
            },
        );
        queue.submit(Some(encoder.finish()));

        let rgba = read_rgba8(device, readback, self.width, self.height)?;
        let color_image = ColorImage::from_rgba_unmultiplied(
            [self.width as usize, self.height as usize],
            &rgba,
        );

        let handle = match self.egui_texture.take() {
            Some(mut existing) => {
                existing.set(color_image, TextureOptions::LINEAR);
                existing
            }
            None => ctx.load_texture(
                "ene-stage-vrm",
                color_image,
                TextureOptions::LINEAR,
            ),
        };
        self.egui_texture = Some(handle);
        Ok(())
    }
}

/// Loaded VRM model with camera, look-at, and expression state.
pub struct CompanionAvatar {
    model: Option<VrmModel>,
    renderer: Option<VrmRenderer>,
    camera: OrthographicCamera,
    model_uniform: ModelUniform,
    look_at_eval: Option<LookAtEvaluator>,
    look_at_target: Vec3,
    look_at_bone: Option<LookAtBoneOutput>,
    idle_time: f32,
    blink_timer: f32,
    blinking: bool,
    #[expect(dead_code, reason = "retained for future hot-reload of renderer targets")]
    surface_format: wgpu::TextureFormat,
}

impl CompanionAvatar {
  #[must_use]
    pub fn new(surface_format: wgpu::TextureFormat) -> Self {
        Self {
            model: None,
            renderer: None,
            camera: OrthographicCamera::default(),
            model_uniform: ModelUniform::default(),
            look_at_eval: None,
            look_at_target: Vec3::new(0.0, 1.0, 0.0),
            look_at_bone: None,
            idle_time: 0.0,
            blink_timer: 3.5,
            blinking: false,
            surface_format,
        }
    }

    #[must_use]
    pub fn is_loaded(&self) -> bool {
        self.model.is_some()
    }

    #[must_use]
    pub fn renderer(&self) -> Option<&VrmRenderer> {
        self.renderer.as_ref()
    }

    #[must_use]
    pub fn camera(&self) -> &OrthographicCamera {
        &self.camera
    }

    #[must_use]
    pub fn model_uniform(&self) -> &ModelUniform {
        &self.model_uniform
    }

    #[must_use]
    pub fn head_world(&mut self) -> Vec3 {
        if let Some(model) = self.model.as_mut() {
            head_world_position(model, &self.model_uniform)
        } else {
            Vec3::new(0.0, 1.4, 0.0)
        }
    }

    pub fn set_model_uniform(&mut self, uniform: ModelUniform) {
        self.model_uniform = uniform;
    }

    /// Load a `.vrm` from disk and build a matching [`VrmRenderer`].
    pub fn load(
        path: &Path,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        surface_format: wgpu::TextureFormat,
    ) -> Result<Self, AvatarError> {
        let model = load_vrm(path, device, queue)?;
        let look_at_eval = model.look_at().map(LookAtEvaluator::new);
        let model_uniform = auto_fit_uniform(&model, &OrthographicCamera::default());
        let renderer = VrmRenderer::new(device, queue, surface_format, None, &model);
        Ok(Self {
            model: Some(model),
            renderer: Some(renderer),
            camera: OrthographicCamera::default(),
            model_uniform,
            look_at_eval,
            look_at_target: Vec3::ZERO,
            look_at_bone: None,
            idle_time: 0.0,
            blink_timer: 3.5,
            blinking: false,
            surface_format,
        })
    }

    /// Write the bundled minimal VRM fixture to `path` (bootstrap / tests).
    pub fn write_default_minimal_vrm(path: &Path) -> VrmResult<()> {
        write_glb(path)
    }

    pub fn set_expression(&mut self, label: &str, intensity: f32) {
        if let Some(model) = self.model.as_mut() {
            let name = ExpressionName::new(label);
            let _ = model.expressions_mut().set_expression(&name, intensity);
        }
    }

    pub fn apply_viseme(&mut self, weights: VisemeWeights) {
        if let Some(model) = self.model.as_mut() {
            model.expressions_mut().apply_viseme_weights(&weights);
        }
    }

    pub fn set_look_at_target(&mut self, world: Vec3) {
        self.look_at_target = world;
        let Some(model) = self.model.as_mut() else {
            return;
        };
        let Some(eval) = self.look_at_eval.as_ref() else {
            return;
        };
        let head_world = head_world_position(model, &self.model_uniform);
        let head_rest = model
            .humanoid
            .head()
            .map_or(glam::Quat::IDENTITY, |entry| entry.rest.rotation);
        match eval.evaluate(head_world, world, head_rest) {
            LookAtOutput::Bone(output) => {
                self.look_at_bone = Some(output);
            }
            LookAtOutput::Expression(expr) => {
                let layer = model.expressions_mut();
                let _ = layer.set_expression(&ExpressionName::new("lookUp"), expr.look_up);
                let _ = layer.set_expression(&ExpressionName::new("lookDown"), expr.look_down);
                let _ = layer.set_expression(&ExpressionName::new("lookLeft"), expr.look_left);
                let _ = layer.set_expression(&ExpressionName::new("lookRight"), expr.look_right);
                self.look_at_bone = None;
            }
            _ => {}
        }
    }

    /// Simple procedural idle: periodic blink when a `blink` expression exists.
    pub fn tick(&mut self, dt: f32) {
        if dt <= 0.0 {
            return;
        }
        self.idle_time += dt;
        self.blink_timer -= dt;
        if self.blink_timer <= 0.0 {
            self.blinking = !self.blinking;
            self.blink_timer = if self.blinking { 0.12 } else { 3.0 + (self.idle_time % 2.0) };
            if let Some(model) = self.model.as_mut() {
                let weight = if self.blinking { 1.0 } else { 0.0 };
                let _ = model
                    .expressions_mut()
                    .set_expression(&ExpressionName::new("blink"), weight);
            }
        }
        self.set_look_at_target(self.look_at_target);
    }

    /// Interpret performance-protocol JSON (`expression` queue entries).
    pub fn apply_body_event(&mut self, value: &serde_json::Value) {
        if let Some(cmd) = value.get("command") {
            self.apply_body_event(cmd);
            return;
        }
        let kind = value
            .get("type")
            .or_else(|| value.get("kind"))
            .and_then(serde_json::Value::as_str);
        let Some(kind) = kind else {
            return;
        };
        if kind == "body.expression" || kind == "expression" {
            let label = value
                .get("label")
                .and_then(serde_json::Value::as_str)
                .or_else(|| value.get("name").and_then(serde_json::Value::as_str));
            let intensity = value
                .get("intensity")
                .and_then(serde_json::Value::as_f64)
                .map(|v| v as f32)
                .or_else(|| {
                    value
                        .get("weight")
                        .and_then(serde_json::Value::as_f64)
                        .map(|v| v as f32)
                })
                .unwrap_or(1.0);
            if let Some(label) = label {
                self.set_expression(label, intensity);
            }
        }
    }

    /// Render into an existing color/depth pair (caller owns textures).
    pub fn render_to_texture(
        &mut self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        color_view: &wgpu::TextureView,
        depth_view: &wgpu::TextureView,
        width: u32,
        height: u32,
    ) -> Result<(), AvatarError> {
        let model = self.model.as_mut().ok_or(AvatarError::NotLoaded)?;
        let renderer = self.renderer.as_ref().ok_or(AvatarError::NotLoaded)?;

        if !model.expressions_meta.is_empty() {
            apply_overrides(&mut model.expressions.weights, &model.expressions_meta);
        }

        let aspect = width.max(1) as f32 / height.max(1) as f32;
        self.camera.set_aspect(aspect);

        let frame = VrmaFrame::default();
        let palette = model.update_skin_palette(&frame, self.look_at_bone.as_ref());
        renderer.update_skin_palette(queue, palette);

        renderer.render(
            queue,
            encoder,
            color_view,
            depth_view,
            model,
            &self.camera,
            &self.model_uniform,
            true,
        );
        Ok(())
    }
}

fn auto_fit_uniform(model: &VrmModel, camera: &OrthographicCamera) -> ModelUniform {
    let (min, max) = model.aabb();
    let scale = camera.compute_auto_fit_scale(min, max, 0.9);
    let center = [
        (min[0] + max[0]) * 0.5,
        (min[1] + max[1]) * 0.5,
        (min[2] + max[2]) * 0.5,
    ];
    ModelUniform::from_position_scale([-center[0] * scale, -center[1] * scale, 0.0], scale)
}

fn head_world_position(model: &mut VrmModel, model_uniform: &ModelUniform) -> Vec3 {
    let frame = VrmaFrame::default();
    let _ = model.update_skin_palette(&frame, None);
    let local = model
        .humanoid
        .head()
        .and_then(|entry| model.nodes.world_positions.get(entry.node).copied())
        .unwrap_or(Vec3::new(0.0, 1.4, 0.0));
    let model_mat = Mat4::from_cols_array_2d(&model_uniform.model);
    model_mat.transform_point3(local)
}

fn padded_bytes_per_row(width: u32) -> u32 {
    let unpadded = width * 4;
    let align = COPY_BYTES_PER_ROW_ALIGNMENT;
    unpadded.div_ceil(align) * align
}

fn read_rgba8(
    device: &wgpu::Device,
    buffer: &wgpu::Buffer,
    width: u32,
    height: u32,
) -> Result<Vec<u8>, AvatarError> {
    let (tx, rx) = mpsc::channel();
    let slice = buffer.slice(..);
    slice.map_async(wgpu::MapMode::Read, move |result| {
        if tx.send(result).is_err() {
            // Readback receiver dropped.
        }
    });
    loop {
        if device.poll(wgpu::PollType::wait_indefinitely()).is_err() {
            return Err(AvatarError::Map);
        }
        if let Ok(result) = rx.try_recv() {
            if result.is_err() {
                return Err(AvatarError::Map);
            }
            break;
        }
    }
    let mapped = slice.get_mapped_range();
    let padded = padded_bytes_per_row(width) as usize;
    let row_bytes = (width * 4) as usize;
    let h = height as usize;
    let mut rgba = Vec::with_capacity(row_bytes * h);
    for row in 0..h {
        let start = row * padded;
        let end = start + row_bytes;
        if end <= mapped.len() {
            rgba.extend_from_slice(&mapped[start..end]);
        }
    }
    drop(mapped);
    buffer.unmap();
    if rgba.is_empty() {
        return Err(AvatarError::Readback);
    }
    Ok(rgba)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use ene_vrm::minimal::minimal_vrm_glb_bytes;
    use tempfile::TempDir;

    use super::CompanionAvatar;

    #[test]
    fn default_minimal_vrm_writes_parseable_glb() {
        let dir = TempDir::new().expect("tempdir");
        let path: PathBuf = dir.path().join("minimal.vrm");
        CompanionAvatar::write_default_minimal_vrm(&path).expect("write");
        let bytes = std::fs::read(&path).expect("read");
        assert_eq!(&bytes[..4], b"glTF");
        assert_eq!(bytes, minimal_vrm_glb_bytes());
    }

    #[test]
    fn apply_body_event_expression_json() {
        let mut avatar = CompanionAvatar::new(wgpu::TextureFormat::Rgba8UnormSrgb);
        let event = serde_json::json!({
            "type": "body.expression",
            "label": "happy",
            "intensity": 0.75
        });
        avatar.apply_body_event(&event);
        let alt = serde_json::json!({
            "kind": "expression",
            "label": "relaxed",
            "intensity": 0.5
        });
        avatar.apply_body_event(&alt);
    }
}
