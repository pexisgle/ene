//! VRM renderer — wgpu render pipeline + bind group layouts that
//! can draw a [`VrmModel`] into a `wgpu::TextureView`.
//!
//! PR3 ships a **basic PBR-lite** shader (`lit + base color`):
//!
//! - Lit lambertian + half-Lambert blend on the base-color texture.
//! - Single directional light from `(0.3, 0.8, 0.5)` in world
//!   space (kept fixed even when the model translates, since the
//!   light direction is now sourced from `world_pos`).
//! - Alpha-blend output (pre-multiplied) so transparent textures
//!   (e.g. MToon's outline-transparent pass) composite correctly.
//! - **PR4.1**: per-frame `ModelUniform` (bind group 1) is applied
//!   between view-proj and the vertex position. The runtime
//!   composes it from `CharacterState::character_position` +
//!   `model_scale`, so the Character settings page X/Y/Z sliders
//!   now move the model in world space.
//!
//! The full MToon shader (rim / matcap / outline / emission) is a
//! follow-up PR.
use crate::camera::{CameraUniform, ModelUniform, OrthographicCamera};
use crate::model::{MeshVertex, VrmModel};

const SHADER_SOURCE: &str = include_str!("shaders/mtoon_lite.wgsl");

/// Render pipeline + bind group layouts for one VRM model.
///
/// Construct once per [`VrmModel`] with [`VrmRenderer::new`]. Call
/// [`VrmRenderer::render`] every frame to draw the model.
pub struct VrmRenderer {
    /// Per-frame camera uniform buffer (group 0).
    camera_buf: wgpu::Buffer,
    /// Per-frame camera bind group (group 0).
    camera_bind_group: wgpu::BindGroup,
    /// Per-frame model uniform buffer (group 1).
    model_buf: wgpu::Buffer,
    /// Per-frame model bind group (group 1).
    model_bind_group: wgpu::BindGroup,
    /// Render pipeline.
    pipeline: wgpu::RenderPipeline,
}

impl VrmRenderer {
    /// Build a renderer for the given model. The model's
    /// base-color texture (if any) is bound at group `(2)`.
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
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

        // Bind group `(1)` — model transform uniform (PR4.1).
        let model_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("vrm.model_buf"),
            size: std::mem::size_of::<ModelUniform>() as wgpu::BufferAddress,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        // Pre-fill the model buffer with identity so the first
        // frame is correct even before the runtime composes a
        // transform. (We do have the queue here now.)
        queue.write_buffer(&model_buf, 0, bytemuck::bytes_of(&ModelUniform::default()));
        let model_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("vrm.model_bgl"),
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
        let model_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("vrm.model_bg"),
            layout: &model_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: model_buf.as_entire_binding(),
            }],
        });

        // The base-color bind group layout comes from the first
        // primitive that has a base color texture (group `(2)`).
        // All primitives share the same layout — just different
        // bind groups — so the pipeline is built once and the
        // renderer binds a different bind group per primitive in
        // the `render` loop. For models where every primitive
        // lacks a base color, fall back to a dummy empty layout.
        let base_color_bgl = model
            .meshes
            .iter()
            .flat_map(|m| m.primitives.iter())
            .find_map(|p| p.base_color.as_ref().map(|t| t.bind_group_layout.clone()))
            .unwrap_or_else(|| {
                device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                    label: Some("vrm.dummy_bgl"),
                    entries: &[],
                })
            });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("vrm.pipeline_layout"),
            bind_group_layouts: &[Some(&camera_bgl), Some(&model_bgl), Some(&base_color_bgl)],
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
                // glTF 2.0 and VRM 1.0 share the same convention:
                // triangles wound CCW when viewed from outside the
                // model. VRoid (Alicia) and other VRM 1.0 humanoid
                // models are exported with their face at `+Z`, so
                // the camera at `(0, 0.3, 3)` looking at the origin
                // already sees the model as front-facing. With that
                // orientation, `CullMode::Back` is the natural
                // choice and halves the fragment work.
                //
                // An earlier 180°-around-Y pre-rotation in
                // `ModelUniform::from_position_scale` was the wrong
                // direction: it showed the back of the character
                // and mirrored `character_state.character_position.x`,
                // which is what was making the model appear shifted
                // to the right and half off-screen.
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
            model_buf,
            model_bind_group,
            pipeline,
        }
    }

    /// Render `model` into `view` with the given camera + model
    /// transform. `depth_view` is the depth attachment (must
    /// match the pipeline's `Depth32Float` format).
    ///
    /// `queue` is used to upload the camera and model uniforms
    /// before the render pass; the encoder is responsible for
    /// the rest. `transparent` controls the clear color.
    pub fn render(
        &self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        depth_view: &wgpu::TextureView,
        model: &VrmModel,
        camera: &OrthographicCamera,
        model_uniform: &ModelUniform,
        transparent: bool,
    ) {
        let camera_uniform = camera
            .uniform()
            .expect("orthographic camera uniform is infallible");
        queue.write_buffer(&self.camera_buf, 0, bytemuck::bytes_of(&camera_uniform));
        queue.write_buffer(&self.model_buf, 0, bytemuck::bytes_of(model_uniform));

        let clear_color = if transparent {
            wgpu::Color::TRANSPARENT
        } else {
            wgpu::Color {
                r: 0.2,
                g: 0.2,
                b: 0.2,
                a: 1.0,
            }
        };

        let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("vrm.pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(clear_color),
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
        rp.set_bind_group(1, &self.model_bind_group, &[]);
        for mesh in &model.meshes {
            for prim in &mesh.primitives {
                if let Some(t) = &prim.base_color {
                    rp.set_bind_group(2, &t.bind_group, &[]);
                }
                rp.set_vertex_buffer(0, prim.vertex_buf.slice(..));
                rp.set_index_buffer(prim.index_buf.slice(..), wgpu::IndexFormat::Uint32);
                rp.draw_indexed(0..prim.index_count, 0, 0..1);
            }
        }
    }
}
