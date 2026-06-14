//! VRM renderer — wgpu render pipeline + bind group layouts that
//! can draw a [`VrmModel`] into a `wgpu::TextureView`.
//!
//! PR3 ships a **basic PBR-lite** shader (`lit + base color`):
//!
//! - Lit lambertian + half-Lambert blend on the base-color texture.
//! - Single directional light from `(0.3, 0.8, 0.5)`.
//! - Alpha-blend output (pre-multiplied) so transparent textures
//!   (e.g. MToon's outline-transparent pass) composite correctly.
//!
//! The full MToon shader (rim / matcap / outline / emission) is a
//! follow-up PR.
use crate::camera::{CameraUniform, OrthographicCamera};
use crate::model::{MeshVertex, VrmModel};

const SHADER_SOURCE: &str = include_str!("shaders/mtoon_lite.wgsl");

/// Render pipeline + bind group layouts for one VRM model.
///
/// Construct once per [`VrmModel`] with [`VrmRenderer::new`]. Call
/// [`VrmRenderer::render`] every frame to draw the model.
pub struct VrmRenderer {
    /// Per-frame camera uniform buffer.
    camera_buf: wgpu::Buffer,
    /// Per-frame camera bind group.
    camera_bind_group: wgpu::BindGroup,
    /// Render pipeline.
    pipeline: wgpu::RenderPipeline,
}

impl VrmRenderer {
    /// Build a renderer for the given model. The model's
    /// base-color texture (if any) is bound at slot `(1)`.
    pub fn new(
        device: &wgpu::Device,
        surface_format: wgpu::TextureFormat,
        model: &VrmModel,
    ) -> Self {
        let camera_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("vrm.camera_buf"),
            size: std::mem::size_of::<CameraUniform>() as wgpu::BufferAddress,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // Bind group layout entry `(0)` — camera uniform.
        let camera_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("vrm.camera_bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let camera_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("vrm.camera_bg"),
            layout: &camera_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: camera_buf.as_entire_binding(),
            }],
        });

        // The base-color bind group layout comes from the texture
        // (slot `(1)`). For models with no base color, fall back to
        // a dummy empty layout — the shader's `has_base_color`
        // uniform branch reads a flat color.
        let base_color_bgl = model
            .base_color
            .as_ref()
            .map(|t| t.bind_group_layout.clone())
            .unwrap_or_else(|| {
                device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                    label: Some("vrm.dummy_bgl"),
                    entries: &[],
                })
            });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("vrm.pipeline_layout"),
            bind_group_layouts: &[Some(&camera_bgl), Some(&base_color_bgl)],
            immediate_size: 0,
        });

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("vrm.shader"),
            source: wgpu::ShaderSource::Wgsl(SHADER_SOURCE.into()),
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("vrm.pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[MeshVertex::LAYOUT],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: Some(wgpu::Face::Back),
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: surface_format,
                    blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });

        Self {
            camera_buf,
            camera_bind_group,
            pipeline,
        }
    }

    /// Render `model` into `view` with the given camera. `depth_view`
    /// is the depth attachment (must match the pipeline's
    /// `Depth32Float` format).
    ///
    /// `queue` is used to upload the camera uniform before the
    /// render pass; the encoder is responsible for the rest.
    pub fn render(
        &self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        depth_view: &wgpu::TextureView,
        model: &VrmModel,
        camera: &OrthographicCamera,
    ) {
        let uniform = camera
            .uniform()
            .expect("orthographic camera uniform is infallible");
        queue.write_buffer(&self.camera_buf, 0, bytemuck::bytes_of(&uniform));

        let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("vrm.pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
                depth_slice: None,
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: depth_view,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.0),
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });

        rp.set_pipeline(&self.pipeline);
        rp.set_bind_group(0, &self.camera_bind_group, &[]);
        if let Some(t) = &model.base_color {
            rp.set_bind_group(1, &t.bind_group, &[]);
        }
        rp.set_vertex_buffer(0, model.mesh.vertex_buf.slice(..));
        rp.set_index_buffer(model.mesh.index_buf.slice(..), wgpu::IndexFormat::Uint32);
        rp.draw_indexed(0..model.mesh.index_count, 0, 0..1);
    }
}
