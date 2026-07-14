//! FXAA post-process pass.
//!
//! The model is first drawn into an intermediate texture sized
//! to match the swapchain; the post-process pass then samples
//! that texture and writes the smoothed output to the
//! swapchain. The pipeline is a fullscreen triangle with
//! `vec2<f32> ∈ {(-1,-1), (3,-1), (-1,3)}` positions and
//! corresponding UVs in `{(0,0), (2,0), (0,2)}`.
use glam::Vec2;
use wgpu::util::DeviceExt;

const FXAA_VERTICES: [PostVertex; 3] = [
    PostVertex {
        position: [-1.0, -1.0],
        uv: [0.0, 0.0],
    },
    PostVertex {
        position: [3.0, -1.0],
        uv: [2.0, 0.0],
    },
    PostVertex {
        position: [-1.0, 3.0],
        uv: [0.0, 2.0],
    },
];

/// Fullscreen-triangle vertex for the post-process pass.
/// `position` is in NDC (`{-1, 3}` × `{-1, 3}` covers the
/// framebuffer with one triangle); `uv` is the corresponding
/// source-texture coordinate.
#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct PostVertex {
    /// NDC position of the vertex.
    pub position: [f32; 2],
    /// Source texture UV.
    pub uv: [f32; 2],
}

/// Uniforms for the FXAA pass. `inv_size` is the inverse of
/// the source texture size in pixels; `_pad` keeps the struct
/// 16-byte aligned for std140-like layouts.
#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct PostUniforms {
    /// 1 / texture size, in pixels. WGSL samples with `uv` in
    /// `[0, 1]`, so the luma offsets are `uv + inv_size *
    /// step`.
    pub inv_size: [f32; 2],
    /// Padding for 16-byte alignment.
    #[expect(clippy::pub_underscore_fields)]
    pub _pad: [f32; 2],
}

/// FXAA post-process. Holds the intermediate texture, the
/// FXAA pipeline, and the per-frame uniforms. Rebuilt by
/// [`PostProcessor::resize`] when the swapchain size changes.
pub struct PostProcessor {
    pipeline: wgpu::RenderPipeline,
    uniform_buffer: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    vertex_buffer: wgpu::Buffer,
    intermediate_view: wgpu::TextureView,
    intermediate_size: (u32, u32),
    intermediate_format: wgpu::TextureFormat,
}

impl PostProcessor {
    /// Build a new post-processor. The intermediate texture
    /// matches the swapchain format so the depth-and-color
    /// interaction stays identical to the no-FXAA path.
    #[must_use]
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        swapchain_format: wgpu::TextureFormat,
        size: (u32, u32),
        shader: &wgpu::ShaderModule,
    ) -> Option<Self> {
        if size.0 == 0 || size.1 == 0 {
            return None;
        }
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("post.sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            ..Default::default()
        });

        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("post.uniforms"),
            size: std::mem::size_of::<PostUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("post.vertices"),
            contents: bytemuck::cast_slice(&FXAA_VERTICES),
            usage: wgpu::BufferUsages::VERTEX,
        });

        let intermediate = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("post.intermediate"),
            size: wgpu::Extent3d {
                width: size.0,
                height: size.1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: swapchain_format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let intermediate_view = intermediate.create_view(&wgpu::TextureViewDescriptor::default());

        // The uniform buffer must outlive the bind group; it
        // is captured by `&'static`-style borrow in the bind
        // group layout. We use a per-frame uniform bind group
        // so the runtime can update `inv_size` without
        // recreating the post-processor.
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("post.bind_group_layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("post.pipeline_layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("post.pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: shader,
                entry_point: Some("vs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<PostVertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2],
                }],
            },
            fragment: Some(wgpu::FragmentState {
                module: shader,
                entry_point: Some("fs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: swapchain_format,
                    blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                strip_index_format: None,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: None,
                polygon_mode: wgpu::PolygonMode::Fill,
                unclipped_depth: false,
                conservative: false,
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("post.uniform_bind_group"),
            layout: &bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&intermediate_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: uniform_buffer.as_entire_binding(),
                },
            ],
        });

        // Initialise the uniforms with the current size.
        queue.write_buffer(
            &uniform_buffer,
            0,
            bytemuck::cast_slice(&[PostUniforms {
                inv_size: [1.0 / size.0 as f32, 1.0 / size.1 as f32],
                _pad: [0.0, 0.0],
            }]),
        );

        Some(Self {
            pipeline,
            uniform_buffer,
            bind_group,
            vertex_buffer,
            intermediate_view,
            intermediate_size: size,
            intermediate_format: swapchain_format,
        })
    }

    /// Re-create the post-processor at a new size. Cheap path
    /// when the size is unchanged (no-op return). The runtime
    /// calls this on `WindowEvent::Resized` (in lock-step with
    /// the depth-texture resize).
    pub fn resize(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        shader: &wgpu::ShaderModule,
        size: (u32, u32),
    ) {
        if size == self.intermediate_size {
            return;
        }
        // Cheap rebuild: same as `new` but in-place. The
        // intermediate texture + bind group must be rebuilt
        // because wgpu textures are immutable.
        if let Some(new) = Self::new(device, queue, self.intermediate_format, size, shader) {
            *self = new;
        }
    }

    /// Run the FXAA pass. `src` is the texture the model
    /// rendered into (must match the post-processor's
    /// intermediate view). `dst` is the swapchain view.
    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        dst: &wgpu::TextureView,
    ) {
        queue.write_buffer(
            &self.uniform_buffer,
            0,
            bytemuck::cast_slice(&[PostUniforms {
                inv_size: [
                    1.0 / self.intermediate_size.0 as f32,
                    1.0 / self.intermediate_size.1 as f32,
                ],
                _pad: [0.0, 0.0],
            }]),
        );
        let _ = device;
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("post.fxaa"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: dst,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
                depth_slice: None,
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind_group, &[]);
        pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
        pass.draw(0..3, 0..1);
    }

    /// The view the model must be rendered into. Returned by
    /// value; the runtime passes it to the model's
    /// `renderer.render` call as the colour attachment.
    #[must_use]
    pub const fn intermediate_view(&self) -> &wgpu::TextureView {
        &self.intermediate_view
    }

    /// The size the intermediate texture was built at.
    #[must_use]
    pub const fn size(&self) -> (u32, u32) {
        self.intermediate_size
    }
}

/// Helper: 1/Vec2 inverse for the `inv_size` uniform. The
/// shader reads `vec2<f32> inv_size`; a typed accessor keeps
/// the post-process `init` and `resize` paths symmetric.
#[must_use]
pub fn inv_size(size: (u32, u32)) -> Vec2 {
    Vec2::new(1.0 / size.0 as f32, 1.0 / size.1 as f32)
}
