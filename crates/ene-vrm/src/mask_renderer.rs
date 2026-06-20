//! PR-LX.7: Solid-color mask render pass for the offscreen
//! Wayland / X11 silhouette capture.
//!
//! Mirrors [`crate::DebugRenderer`] in shape, but skips the
//! per-frame vertex buffer: the per-primitive `vertex_buf` /
//! `index_buf` live on the [`crate::VrmPrimitive`] and we bind
//! them in-place. The pipeline is a single solid-color draw
//! per primitive with the orthographic character camera
//! (`view_proj`) and the model's world matrix
//! ([`crate::ModelUniform`]).
//!
//! Output format matches
//! [`crate::wayland_mask_capture::MaskCaptureCamera::MASK_TARGET_FORMAT`]
//! (`Rgba8Unorm`). No depth test, no blending — we want a flat
//! "is the fragment inside the silhouette?" mask that
//! [`crate::wayland_mask_capture::MaskCaptureCamera::extract_rectangles`]
//! can read back on the CPU.

use crate::camera::{CameraUniform, ModelUniform};
use crate::model::VrmModel;

/// Solid-color mask render pass. Owns the wgpu pipeline +
/// per-frame uniform buffers; the per-primitive `vertex_buf` /
/// `index_buf` are borrowed from the [`VrmModel`] at draw time.
pub struct MaskRenderer {
    pipeline: wgpu::RenderPipeline,
    camera_buf: wgpu::Buffer,
    camera_bind_group: wgpu::BindGroup,
    model_buf: wgpu::Buffer,
    model_bind_group: wgpu::BindGroup,
    /// `target_format` is cached so `render` can refuse to draw
    /// when the caller's colour view format does not match (we
    /// build the pipeline against the offscreen mask target's
    /// `Rgba8Unorm` once at construction time).
    target_format: wgpu::TextureFormat,
}

const MASK_SHADER: &str = include_str!("shaders/mask.wgsl");

impl MaskRenderer {
    /// Build a new mask pipeline against the given target
    /// colour format (the mask render target's format, e.g.
    /// `Rgba8Unorm`).
    pub fn new(device: &wgpu::Device, target_format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("mask.shader"),
            source: wgpu::ShaderSource::Wgsl(MASK_SHADER.into()),
        });

        // (0) camera_uniform — CameraUniform buffer.
        let camera_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("mask.camera_bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: wgpu::BufferSize::new(
                        std::mem::size_of::<CameraUniform>() as u64
                    ),
                },
                count: None,
            }],
        });

        // (1) model_uniform — ModelUniform buffer.
        let model_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("mask.model_bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: wgpu::BufferSize::new(
                        std::mem::size_of::<ModelUniform>() as u64
                    ),
                },
                count: None,
            }],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("mask.pipeline_layout"),
            bind_group_layouts: &[Some(&camera_bgl), Some(&model_bgl)],
            immediate_size: 0,
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("mask.pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<[f32; 3]>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x3],
                }],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: target_format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                strip_index_format: None,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: None,
                unclipped_depth: false,
                polygon_mode: wgpu::PolygonMode::Fill,
                conservative: false,
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        let camera_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("mask.camera_buf"),
            size: std::mem::size_of::<CameraUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let camera_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("mask.camera_bg"),
            layout: &camera_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: camera_buf.as_entire_binding(),
            }],
        });

        let model_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("mask.model_buf"),
            size: std::mem::size_of::<ModelUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let model_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("mask.model_bg"),
            layout: &model_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 1, // matches `model_bgl` binding index above
                resource: model_buf.as_entire_binding(),
            }],
        });

        Self {
            pipeline,
            camera_buf,
            camera_bind_group,
            model_buf,
            model_bind_group,
            target_format,
        }
    }

    /// The texture format the pipeline was built against.
    pub fn target_format(&self) -> wgpu::TextureFormat {
        self.target_format
    }

    /// Record a mask pass into `encoder`. The render pass
    /// clears `target_view` to `(0, 0, 0, 0)` and draws every
    /// primitive of `model` as a solid `(1, 1, 1, 1)` triangle
    /// list, transformed by `model_uniform` and projected with
    /// `camera.view_proj`. No depth test, no blending.
    pub fn render(
        &self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        target_view: &wgpu::TextureView,
        model: &VrmModel,
        camera_uniform: &CameraUniform,
        model_uniform: &ModelUniform,
    ) {
        queue.write_buffer(&self.camera_buf, 0, bytemuck::bytes_of(camera_uniform));
        queue.write_buffer(&self.model_buf, 0, bytemuck::bytes_of(model_uniform));

        let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("mask.render_pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target_view,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: 0.0,
                        g: 0.0,
                        b: 0.0,
                        a: 0.0,
                    }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });

        rp.set_pipeline(&self.pipeline);
        rp.set_bind_group(0, &self.camera_bind_group, &[]);
        rp.set_bind_group(1, &self.model_bind_group, &[]);

        for mesh in &model.meshes {
            for prim in &mesh.primitives {
                if prim.index_count == 0 || prim.vertex_count == 0 {
                    continue;
                }
                rp.set_vertex_buffer(0, prim.vertex_buf.slice(..));
                rp.set_index_buffer(prim.index_buf.slice(..), wgpu::IndexFormat::Uint32);
                rp.draw_indexed(0..prim.index_count, 0, 0..1);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// MaskRenderer has no `Drop` and is `Send + Sync` because
    /// all fields are `wgpu` handles.
    #[test]
    fn mask_renderer_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<MaskRenderer>();
    }
}
