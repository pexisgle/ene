//! VRM renderer — wgpu render pipeline + bind group layouts that
//! can draw a [`VrmModel`] into a `wgpu::TextureView`.
//!
//! Ships a PBR-lite shader (`lit + base color`) for primitives
//! without the `VRMC_materials_mtoon` extension and the full
//! `MToon` shader (`shaders/mtoon_full.wgsl`) for primitives that
//! carry it:
//!
//! - Lit lambertian + half-Lambert blend on the base-color texture.
//! - Single directional light from `(0.3, 0.8, 0.5)` in world
//!   space (kept fixed even when the model translates, since the
//!   light direction is sourced from `world_pos`).
//! - Alpha-blend output (pre-multiplied) so transparent textures
//!   (e.g. `MToon`'s outline-transparent pass) composite correctly.
//! - Per-frame `ModelUniform` (bind group 1) is applied between
//!   view-proj and the vertex position. The runtime composes it
//!   from `CharacterState::character_position` + `model_scale`,
//!   so the Character settings page X/Y/Z sliders move the model
//!   in world space.
//! - Bind group `(3)` carries the morph-target data (storage +
//!   uniform). Primitives that define morph targets get a
//!   per-primitive storage buffer (the position displacements,
//!   in the vertex buffer's raw space) and a uniform
//!   [`PrimitiveMorphMeta`] that the renderer fills every frame
//!   from the model's global `BTreeMap<ExpressionName, f32>`
//!   weight map. Primitives without morph targets bind a shared
//!   dummy layout with `target_count = 0`; the shader's
//!   `if (target_count > 0u)` early-out makes the cost near
//!   zero. The slot index used to look up weights is the
//!   **per-primitive local** index (the position of the target
//!   in `PrimitiveMorphs::targets`) — not a global
//!   name-flattened index — so the shader's
//!   `weights[t / 4][t % 4]` lookup always matches the
//!   corresponding row in the offsets storage buffer.
use wgpu::util::DeviceExt;

use crate::camera::{CameraUniform, ModelUniform, OrthographicCamera};
use crate::expression::{PrimitiveMorphMeta, PrimitiveMorphs};
use crate::model::{AlphaMode, VrmModel};

const SHADER_SOURCE: &str = include_str!("shaders/mtoon_skinned.wgsl");
const UNLIT_SHADER_SOURCE: &str = include_str!("shaders/unlit_skinned.wgsl");
const MTOON_SHADER_SOURCE: &str = include_str!("shaders/mtoon_full.wgsl");

/// Number of skin-matrix palette slots to allocate when the
/// loaded model has no skin at all. A one-element palette of
/// `Mat4::IDENTITY` is enough because the default `MeshVertex`
/// falls back to `joints = [0, 0, 0, 0]` and `weights = [1, 0, 0, 0]`.
const IDENTITY_SKIN_PALETTE_LEN: usize = 1;

/// Per-primitive GPU resources backing the morph bind group.
///
/// One instance per primitive that has at least one morph target.
/// Primitives without morphs share a single [`DummyMorphGpu`]
/// installed on the renderer at construction time.
struct MorphGpu {
    /// `storage<read>` array of `vec3<f32>` (one entry per
    /// `(target, vertex)` pair). Uploaded once at
    /// [`VrmRenderer::new`] from the loader's normalized
    /// displacement data.
    #[expect(
        dead_code,
        reason = "GPU buffer kept alive while bind group references it"
    )]
    offsets_buf: wgpu::Buffer,
    /// Per-frame uniform that the renderer writes from the
    /// model's global weight map. See [`PrimitiveMorphMeta`].
    meta_buf: wgpu::Buffer,
    /// Pre-built bind group combining `offsets_buf` and
    /// `meta_buf`. Set as group `(3)` for every draw of the
    /// owning primitive.
    bind_group: wgpu::BindGroup,
    /// Cached copy of the primitive's target count. Used by the
    /// render loop to decide whether to re-pack the meta
    /// uniform.
    target_count: u32,
    vertex_count: u32,
}

/// Shared dummy morph bind group used by every primitive without
/// morph targets. Keeps the pipeline layout (which always
/// declares group `(3)`) consistent without paying for per-
/// primitive dummy storage allocations.
struct DummyMorphGpu {
    /// Single `vec4` storage entry. Never read by the shader
    /// because the bound `meta_buf` has `target_count = 0`.
    #[expect(
        dead_code,
        reason = "GPU buffer kept alive while bind group references it"
    )]
    offsets_buf: wgpu::Buffer,
    /// Meta uniform with `target_count = 0` and zero weights.
    #[expect(
        dead_code,
        reason = "GPU buffer kept alive while bind group references it"
    )]
    meta_buf: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
}

/// 1×1 white texture bound at group `(2)` when a primitive has no
/// base-color image. The unlit/MToon shaders always declare that
/// group, so an empty layout panics on pipeline create.
struct DummyBaseColorGpu {
    #[expect(
        dead_code,
        reason = "GPU texture kept alive while bind group references it"
    )]
    texture: wgpu::Texture,
    #[expect(
        dead_code,
        reason = "GPU sampler kept alive while bind group references it"
    )]
    sampler: wgpu::Sampler,
    bind_group: wgpu::BindGroup,
}

/// Per-model skin-matrix palette. One `mat4x4<f32>` per
/// joint, uploaded once at construction time with the
/// pre-baked `bind_matrices` (i.e. `inverse_bind[i].inverse()`).
/// Overwritten every frame with the current joint world
/// transforms to drive animation, look-at, and spring bones.
struct SkinGpu {
    matrices_buf: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    /// Number of joints (i.e. the palette length). Used by
    /// callers that want to grow the storage when the loader
    /// finds more joints than expected (defensive — the loader
    /// always produces a fixed-size buffer).
    joint_count: u32,
}

/// Per-primitive `MToon` uniform buffer (group 5).
struct MToonUniformGpu {
    bind_group: wgpu::BindGroup,
}

/// A single entry in the cached draw order. Computed once at
/// renderer construction; the draw order (opaque before
/// transparent) is determined at load time and never changes.
#[derive(Copy, Clone)]
struct DrawItem {
    linear_index: usize,
    alpha_mode: AlphaMode,
    unlit: bool,
}

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
    /// Opaque + mask render pipeline. Depth write on, no
    /// blending. Used for [`AlphaMode::Opaque`] and
    /// [`AlphaMode::Mask`] primitives.
    pipeline_opaque: wgpu::RenderPipeline,
    /// Transparent render pipeline. Depth write off,
    /// pre-multiplied alpha blending. Used for
    /// [`AlphaMode::Blend`] primitives, drawn after all
    /// opaque/mask primitives so the depth buffer is already
    /// populated and transparent surfaces are correctly
    /// occluded by opaque geometry.
    pipeline_transparent: wgpu::RenderPipeline,
    /// unlit opaque pipeline. Same depth state as
    /// `pipeline_opaque` but uses the unlit shader (no lighting).
    pipeline_unlit_opaque: wgpu::RenderPipeline,
    /// unlit transparent pipeline. Same blend state
    /// as `pipeline_transparent` but uses the unlit shader.
    pipeline_unlit_transparent: wgpu::RenderPipeline,
    /// Dummy morph resources, bound for primitives that have no
    /// morph targets.
    dummy_morph: DummyMorphGpu,
    dummy_base_color: DummyBaseColorGpu,
    /// Per-primitive morph GPU resources, aligned 1:1 with
    /// the renderer's draw loop (mesh-major, then primitive-
    /// within-mesh). `None` for primitives that have no
    /// morph targets; the renderer binds `dummy_morph` in
    /// that case.
    morph_gpu: Vec<Option<MorphGpu>>,
    /// Scratch `PrimitiveMorphMeta` reused every frame to
    /// avoid a per-draw allocation. Default-initialised to
    /// all zeros.
    meta_scratch: PrimitiveMorphMeta,
    /// Skin-matrix palette (group 4). Uploaded once
    /// with the model's `bind_matrices` (rest pose) and
    /// overwritten every frame with the current joint world
    /// transforms.
    skin: SkinGpu,
    /// `MToon` per-material uniform buffer (group 5).
    /// One buffer per primitive that has `MToon`; `None` for
    /// primitives that use the lite shader.
    mtoon_uniforms: Vec<Option<MToonUniformGpu>>,
    /// Cached draw order, sorted opaque/mask before transparent.
    /// Built once at construction — the order is determined at
    /// load time and never changes, so sorting every frame is
    /// wasted work.
    draw_order: Vec<DrawItem>,
    pipeline_mtoon_opaque: wgpu::RenderPipeline,
    pipeline_mtoon_transparent: wgpu::RenderPipeline,
    /// Mask render pipeline. Compiles against `mask_format` if provided,
    /// otherwise `None`.
    pipeline_mask: Option<wgpu::RenderPipeline>,
}

impl VrmRenderer {
    /// Build a renderer for the given model. The model's
    /// base-color texture (if any) is bound at group `(2)`.
    /// Morph-target data is bound at group `(3)` on a
    /// per-primitive basis.
    #[must_use]
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        surface_format: wgpu::TextureFormat,
        mask_format: Option<wgpu::TextureFormat>,
        model: &VrmModel,
    ) -> Self {
        let camera_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("vrm.camera_buf"),
            size: std::mem::size_of::<CameraUniform>() as wgpu::BufferAddress,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

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

        let model_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("vrm.model_buf"),
            size: std::mem::size_of::<ModelUniform>() as wgpu::BufferAddress,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        // Pre-fill the model buffer with identity so the first
        // frame is correct even before the runtime composes a
        // transform.
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
        // the `render` loop. Untextured models get a 1×1 white
        // dummy so the shader's group 2 still matches.
        let base_color_bgl = model
            .meshes
            .iter()
            .flat_map(|m| m.primitives.iter())
            .find_map(|p| p.base_color.as_ref().map(|t| t.bind_group_layout.clone()))
            .unwrap_or_else(|| base_color_bind_group_layout(device));
        let dummy_base_color = build_dummy_base_color_gpu(device, queue, &base_color_bgl);

        let morph_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("vrm.morph_bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(PrimitiveMorphMeta::SIZE),
                    },
                    count: None,
                },
            ],
        });

        let skin_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("vrm.skin_bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });

        let mtoon_uniform_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("vrm.mtoon_uniform_bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: wgpu::BufferSize::new(
                        crate::mtoon::MToonUniform::SIZE as u64,
                    ),
                },
                count: None,
            }],
        });

        // Bind group (6) — MToon textures (14 bindings: 7 tex + 7 smp).
        let mtoon_textures_bgl =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("vrm.mtoon_textures_bgl"),
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
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 3,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 4,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 5,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 6,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 7,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 8,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 9,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 10,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 11,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 12,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 13,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        count: None,
                    },
                ],
            });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("vrm.pipeline_layout"),
            bind_group_layouts: &[
                Some(&camera_bgl),
                Some(&model_bgl),
                Some(&base_color_bgl),
                Some(&morph_bgl),
                Some(&skin_bgl),
            ],
            immediate_size: 0,
        });

        // MToon pipeline layout (7 bind groups).
        let mtoon_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("vrm.mtoon_pipeline_layout"),
                bind_group_layouts: &[
                    Some(&camera_bgl),
                    Some(&model_bgl),
                    Some(&base_color_bgl),
                    Some(&morph_bgl),
                    Some(&skin_bgl),
                    Some(&mtoon_uniform_bgl),
                    Some(&mtoon_textures_bgl),
                ],
                immediate_size: 0,
            });

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("vrm.shader"),
            source: wgpu::ShaderSource::Wgsl(SHADER_SOURCE.into()),
        });

        let unlit_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("vrm.unlit_shader"),
            source: wgpu::ShaderSource::Wgsl(UNLIT_SHADER_SOURCE.into()),
        });

        let mtoon_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("vrm.mtoon_shader"),
            source: wgpu::ShaderSource::Wgsl(MTOON_SHADER_SOURCE.into()),
        });

        let pipeline_opaque = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("vrm.pipeline_opaque"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[crate::model::MeshVertex::LAYOUT],
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
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });

        let pipeline_transparent = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("vrm.pipeline_transparent"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[crate::model::MeshVertex::LAYOUT],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: Some(wgpu::Face::Back),
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: Some(false),
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

        // Unlit opaque pipeline. Same layout and
        // depth state as the lit opaque pipeline, but the
        // fragment shader outputs base color directly without
        // any half-Lambert lighting term.
        let pipeline_unlit_opaque =
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("vrm.pipeline_unlit_opaque"),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &unlit_shader,
                    entry_point: Some("vs_main"),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    buffers: &[crate::model::MeshVertex::LAYOUT],
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
                    module: &unlit_shader,
                    entry_point: Some("fs_main"),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: surface_format,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            });

        // Unlit transparent pipeline. Same blend
        // state as the lit transparent pipeline but without
        // lighting.
        let pipeline_unlit_transparent =
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("vrm.pipeline_unlit_transparent"),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &unlit_shader,
                    entry_point: Some("vs_main"),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    buffers: &[crate::model::MeshVertex::LAYOUT],
                },
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    front_face: wgpu::FrontFace::Ccw,
                    cull_mode: Some(wgpu::Face::Back),
                    ..Default::default()
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: wgpu::TextureFormat::Depth32Float,
                    depth_write_enabled: Some(false),
                    depth_compare: Some(wgpu::CompareFunction::Less),
                    stencil: wgpu::StencilState::default(),
                    bias: wgpu::DepthBiasState::default(),
                }),
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &unlit_shader,
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

        let pipeline_mtoon_opaque =
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("vrm.pipeline_mtoon_opaque"),
                layout: Some(&mtoon_pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &mtoon_shader,
                    entry_point: Some("vs_main"),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    buffers: &[crate::model::MeshVertex::LAYOUT],
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
                    module: &mtoon_shader,
                    entry_point: Some("fs_main"),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: surface_format,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            });

        let pipeline_mtoon_transparent =
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("vrm.pipeline_mtoon_transparent"),
                layout: Some(&mtoon_pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &mtoon_shader,
                    entry_point: Some("vs_main"),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    buffers: &[crate::model::MeshVertex::LAYOUT],
                },
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    front_face: wgpu::FrontFace::Ccw,
                    cull_mode: Some(wgpu::Face::Back),
                    ..Default::default()
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: wgpu::TextureFormat::Depth32Float,
                    depth_write_enabled: Some(false),
                    depth_compare: Some(wgpu::CompareFunction::Less),
                    stencil: wgpu::StencilState::default(),
                    bias: wgpu::DepthBiasState::default(),
                }),
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &mtoon_shader,
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

        // Build the per-primitive morph GPU resources.
        // The linear order matches the renderer's draw loop:
        // mesh-major, then primitive-within-mesh, skipping any
        // primitives that the loader dropped (e.g. non-triangle
        // topology).
        let mut morph_gpu: Vec<Option<MorphGpu>> = Vec::new();
        for (idx, prim_morphs) in model.expressions().per_primitive.iter().enumerate() {
            morph_gpu.push(
                prim_morphs
                    .as_ref()
                    .map(|p| build_morph_gpu(device, &morph_bgl, p, idx)),
            );
        }

        let dummy_morph = build_dummy_morph_gpu(device, queue, &morph_bgl);

        let skin = build_skin_gpu(device, queue, &skin_bgl, model);

        let mut mtoon_uniforms: Vec<Option<MToonUniformGpu>> = Vec::new();
        for mesh in &model.meshes {
            for prim in &mesh.primitives {
                if let Some(mat) = &prim.mtoon {
                    let has_base_color = prim.base_color.is_some();
                    let tex_flags = crate::mtoon::texture_flags(mat, has_base_color);
                    let uniform = crate::mtoon::MToonUniform::from_material(mat, tex_flags, 0.0);
                    let uniform_buf =
                        device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                            label: Some("vrm.mtoon_uniform"),
                            contents: bytemuck::bytes_of(&uniform),
                            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                        });
                    let uniform_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
                        label: Some("vrm.mtoon_uniform_bg"),
                        layout: &mtoon_uniform_bgl,
                        entries: &[wgpu::BindGroupEntry {
                            binding: 0,
                            resource: uniform_buf.as_entire_binding(),
                        }],
                    });
                    mtoon_uniforms.push(Some(MToonUniformGpu {
                        bind_group: uniform_bg,
                    }));
                } else {
                    mtoon_uniforms.push(None);
                }
            }
        }
        let pipeline_mask = mask_format.map(|fmt| {
            let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("vrm.mask_shader"),
                source: wgpu::ShaderSource::Wgsl(include_str!("shaders/mask.wgsl").into()),
            });
            // The mask pipeline uses the exact same bind groups as MToon/Unlit,
            // but we bind a dummy texture at group 2 to satisfy wgpu layout requirements.
            let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("vrm.mask_pipeline_layout"),
                bind_group_layouts: &[
                    Some(&camera_bgl),
                    Some(&model_bgl),
                    None,
                    Some(&morph_bgl),
                    Some(&skin_bgl),
                ],
                immediate_size: 0,
            });
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("vrm.pipeline_mask"),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_main"),
                    buffers: &[crate::model::MeshVertex::LAYOUT],
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs_main"),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: fmt,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                }),
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    cull_mode: None,
                    ..Default::default()
                },
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            })
        });

        // Build the cached draw order once. The order (opaque/mask
        // before transparent) is determined at load time and never
        // changes, so we sort here instead of every frame.
        let mut draw_order: Vec<DrawItem> = Vec::new();
        {
            let mut idx: usize = 0;
            for mesh in &model.meshes {
                for prim in &mesh.primitives {
                    draw_order.push(DrawItem {
                        linear_index: idx,
                        alpha_mode: prim.alpha_mode,
                        unlit: prim.unlit,
                    });
                    idx += 1;
                }
            }
        }
        draw_order.sort_by_key(|d| d.alpha_mode.render_phase());

        Self {
            camera_buf,
            camera_bind_group,
            model_buf,
            model_bind_group,
            pipeline_opaque,
            pipeline_transparent,
            pipeline_unlit_opaque,
            pipeline_unlit_transparent,
            dummy_morph,
            dummy_base_color,
            morph_gpu,
            meta_scratch: PrimitiveMorphMeta::default(),
            skin,
            mtoon_uniforms,
            draw_order,
            pipeline_mtoon_opaque,
            pipeline_mtoon_transparent,
            pipeline_mask,
        }
    }

    /// Render `model` into `view` with the given camera + model
    /// transform. `depth_view` is the depth attachment (must
    /// match the pipeline's `Depth32Float` format).
    ///
    /// `queue` is used to upload the camera, model, and morph
    /// uniforms before the render pass; the encoder is
    /// responsible for the rest. `transparent` controls the
    /// clear color when `clear` is true. Subsequent bodies in the
    /// same overlay frame use `clear = false` so they composite.
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
        clear: bool,
    ) {
        // `OrthographicCamera::uniform` is infallible by construction: it
        // only builds a view-proj matrix from already-validated fields and
        // has no `Err` return path. The `VrmResult` wrapper exists for API
        // symmetry with fallible camera accessors.
        #[expect(
            clippy::expect_used,
            reason = "Camera::uniform is infallible by construction"
        )]
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

        let color_load = if clear {
            wgpu::LoadOp::Clear(clear_color)
        } else {
            wgpu::LoadOp::Load
        };
        let depth_load = if clear {
            wgpu::LoadOp::Clear(1.0)
        } else {
            wgpu::LoadOp::Load
        };
        let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("vrm.pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: color_load,
                    store: wgpu::StoreOp::Store,
                },
                depth_slice: None,
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: depth_view,
                depth_ops: Some(wgpu::Operations {
                    load: depth_load,
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });

        rp.set_pipeline(&self.pipeline_opaque);
        rp.set_bind_group(0, &self.camera_bind_group, &[]);
        rp.set_bind_group(1, &self.model_bind_group, &[]);
        rp.set_bind_group(4, &self.skin.bind_group, &[]);

        let all_prims: Vec<&_> = model
            .meshes
            .iter()
            .flat_map(|m| m.primitives.iter())
            .collect();

        for item in self
            .draw_order
            .iter()
            .filter(|d| d.alpha_mode.render_phase() == 0)
        {
            let prim = all_prims[item.linear_index];
            if prim.mtoon.is_some() {
                rp.set_pipeline(&self.pipeline_mtoon_opaque);
            } else if item.unlit {
                rp.set_pipeline(&self.pipeline_unlit_opaque);
            } else {
                rp.set_pipeline(&self.pipeline_opaque);
            }
            self.draw_primitive(&mut rp, queue, model, prim, item.linear_index);
        }

        // Drawn in declaration order (which is
        // roughly back-to-front for most humanoid VRM models);
        // a proper view-Z depth sort is a follow-up.
        for item in self
            .draw_order
            .iter()
            .filter(|d| d.alpha_mode.render_phase() == 1)
        {
            let prim = all_prims[item.linear_index];
            if prim.mtoon.is_some() {
                rp.set_pipeline(&self.pipeline_mtoon_transparent);
            } else if item.unlit {
                rp.set_pipeline(&self.pipeline_unlit_transparent);
            } else {
                rp.set_pipeline(&self.pipeline_transparent);
            }
            self.draw_primitive(&mut rp, queue, model, prim, item.linear_index);
        }
    }

    /// Renders the mask into the provided `target_view` using the
    /// internal `pipeline_mask`. If the renderer was built without
    /// `mask_format`, this is a no-op.
    pub fn render_mask(
        &self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        model: &VrmModel,
        camera_uniform: &CameraUniform,
        model_uniform: &ModelUniform,
    ) {
        let Some(pipeline) = &self.pipeline_mask else {
            return;
        };

        // We can overwrite the camera and model uniforms here because
        // `render_mask` is called sequentially with `render`.
        queue.write_buffer(&self.camera_buf, 0, bytemuck::bytes_of(camera_uniform));
        queue.write_buffer(&self.model_buf, 0, bytemuck::bytes_of(model_uniform));

        let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("vrm.mask_pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
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

        rp.set_pipeline(pipeline);
        rp.set_bind_group(0, &self.camera_bind_group, &[]);
        rp.set_bind_group(1, &self.model_bind_group, &[]);
        rp.set_bind_group(4, &self.skin.bind_group, &[]);

        for (idx, prim) in model
            .meshes
            .iter()
            .flat_map(|m| m.primitives.iter())
            .enumerate()
        {
            if let Some(morph) = self.morph_gpu.get(idx).and_then(Option::as_ref) {
                if let Some(prim_morphs) = model
                    .expressions()
                    .per_primitive
                    .get(idx)
                    .and_then(Option::as_ref)
                {
                    self.upload_morph_meta(queue, morph, prim_morphs, model);
                }
                rp.set_bind_group(3, &morph.bind_group, &[]);
            } else {
                rp.set_bind_group(3, &self.dummy_morph.bind_group, &[]);
            }

            rp.set_vertex_buffer(0, prim.vertex_buf.slice(..));
            rp.set_index_buffer(prim.index_buf.slice(..), wgpu::IndexFormat::Uint32);
            rp.draw_indexed(0..prim.index_count, 0, 0..1);
        }
    }

    fn draw_primitive(
        &self,
        rp: &mut wgpu::RenderPass,
        queue: &wgpu::Queue,
        model: &VrmModel,
        prim: &crate::model::VrmPrimitive,
        linear_index: usize,
    ) {
        if let Some(t) = &prim.base_color {
            rp.set_bind_group(2, &t.bind_group, &[]);
        } else {
            rp.set_bind_group(2, &self.dummy_base_color.bind_group, &[]);
        }
        if let Some(morph) = self.morph_gpu.get(linear_index).and_then(Option::as_ref) {
            if let Some(prim_morphs) = model
                .expressions()
                .per_primitive
                .get(linear_index)
                .and_then(Option::as_ref)
            {
                self.upload_morph_meta(queue, morph, prim_morphs, model);
            }
            rp.set_bind_group(3, &morph.bind_group, &[]);
        } else {
            rp.set_bind_group(3, &self.dummy_morph.bind_group, &[]);
        }
        if let Some(mtoon) = self
            .mtoon_uniforms
            .get(linear_index)
            .and_then(Option::as_ref)
        {
            rp.set_bind_group(5, &mtoon.bind_group, &[]);
        }
        if let Some(textures) = &prim.mtoon_textures {
            rp.set_bind_group(6, &textures.combined_bind_group, &[]);
        }
        rp.set_vertex_buffer(0, prim.vertex_buf.slice(..));
        rp.set_index_buffer(prim.index_buf.slice(..), wgpu::IndexFormat::Uint32);
        rp.draw_indexed(0..prim.index_count, 0, 0..1);
    }

    /// Build the per-frame [`PrimitiveMorphMeta`] uniform for
    /// `morph` from the model's global weight map and upload it.
    ///
    /// The slot index used to look up weights is the
    /// **per-primitive local** index (the position of the
    /// target in `prim_morphs.targets`) — not a model-wide
    /// name-flattened index. The shader's `weights[t / 4][t % 4]`
    /// layout mirrors that local order, and the offsets storage
    /// buffer is filled in the same order at
    /// [`build_morph_gpu`]. Anything past
    /// `MAX_MORPH_TARGETS_PER_PRIMITIVE` is skipped (with a
    /// warning) so a malformed model does not overwrite the
    /// uniform header.
    fn upload_morph_meta(
        &self,
        queue: &wgpu::Queue,
        morph: &MorphGpu,
        prim_morphs: &PrimitiveMorphs,
        model: &VrmModel,
    ) {
        let mut meta = self.meta_scratch;
        meta.vertex_count = morph.vertex_count;
        meta.target_count = morph.target_count;
        for (slot, target) in prim_morphs.targets.iter().enumerate() {
            let slot = slot as u32;
            if (slot as usize) >= crate::expression::MAX_MORPH_TARGETS_PER_PRIMITIVE {
                tracing::warn!(
                    "primitive {:?} has more than {} morph targets; the extras are ignored",
                    prim_morphs.primitive_id,
                    crate::expression::MAX_MORPH_TARGETS_PER_PRIMITIVE,
                );
                break;
            }
            let weight = model
                .expressions()
                .morph_target_weights
                .get(&(prim_morphs.node_index, target.target_index))
                .copied()
                .unwrap_or(0.0);
            let vec4_idx = (slot / 4) as usize;
            let comp = (slot % 4) as usize;
            meta.weights[vec4_idx][comp] = weight;
        }
        queue.write_buffer(&morph.meta_buf, 0, bytemuck::bytes_of(&meta));
    }

    /// Overwrite the skin-palette storage buffer with
    /// the joint world transforms returned by
    /// [`VrmModel::update_skin_palette`].
    ///
    /// `palette` is a `Vec<Mat4>` of length
    /// `VrmModel::joint_count()`. The renderer copies the
    /// 64 bytes-per-matrix data straight into
    /// `self.skin.matrices_buf` via `queue.write_buffer`; the
    /// bind group was created at construction and is
    /// reused, so no pipeline rebuild is needed.
    ///
    /// No-op when the model has no skin (the renderer was
    /// built with the identity one-element palette) or
    /// when `palette.len()` is zero. The runtime should
    /// not call this on every frame for an unskinned
    /// model — the GPU write would be wasted.
    pub fn update_skin_palette(&self, queue: &wgpu::Queue, palette: &[glam::Mat4]) {
        if palette.is_empty() || self.skin.joint_count == 0 {
            return;
        }
        debug_assert_eq!(
            palette.len() as u32,
            self.skin.joint_count,
            "palette length must match the renderer's joint_count"
        );
        queue.write_buffer(&self.skin.matrices_buf, 0, bytemuck::cast_slice(palette));
    }

    /// The joint count of the renderer's skin
    /// palette. Zero for models built with the identity
    /// one-element palette (no skin).
    pub const fn skin_joint_count(&self) -> u32 {
        self.skin.joint_count
    }
}

/// Build a one-shot morph bind group for a single primitive that
/// has at least one morph target. The storage buffer is uploaded
/// once with the loader's displacement data; the meta
/// uniform is rewritten every frame by
/// [`VrmRenderer::upload_morph_meta`].
///
/// `bgl` is the shared morph bind group layout installed on the
/// renderer (and on every other per-primitive bind group). All
/// morph bind groups in the model share this layout so the
/// pipeline layout only needs to reference it once.
fn build_morph_gpu(
    device: &wgpu::Device,
    bgl: &wgpu::BindGroupLayout,
    prim_morphs: &PrimitiveMorphs,
    linear_index: usize,
) -> MorphGpu {
    // Pack the displacements as `vec4` entries so the GPU sees
    // a 16-byte stride per element. WGSL's `vec3` in a storage
    // buffer also has 16-byte alignment, so the shader-side
    // `array<vec3<f32>>` indexing is correct; we just pad each
    // entry with a 0 in `.w` to satisfy the alignment on the
    // host side.
    let total = prim_morphs.uniform_buffer_len as usize * prim_morphs.vertex_count as usize;
    let mut offsets: Vec<[f32; 4]> = Vec::with_capacity(total);
    for target in &prim_morphs.targets {
        for v in &target.position_offsets {
            offsets.push([v[0], v[1], v[2], 0.0]);
        }
    }
    let offsets_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some(&format!("vrm.morph_offsets[{linear_index}]")),
        contents: bytemuck::cast_slice(&offsets),
        usage: wgpu::BufferUsages::STORAGE,
    });

    let meta_buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(&format!("vrm.morph_meta[{linear_index}]")),
        size: PrimitiveMorphMeta::SIZE,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some(&format!("vrm.morph_bg[{linear_index}]")),
        layout: bgl,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: offsets_buf.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: meta_buf.as_entire_binding(),
            },
        ],
    });

    MorphGpu {
        offsets_buf,
        meta_buf,
        bind_group,
        target_count: prim_morphs.uniform_buffer_len,
        vertex_count: prim_morphs.vertex_count,
    }
}

fn base_color_bind_group_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("vrm.dummy_base_color.bgl"),
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
        ],
    })
}

fn build_dummy_base_color_gpu(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    layout: &wgpu::BindGroupLayout,
) -> DummyBaseColorGpu {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("vrm.dummy_base_color"),
        size: wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &[255, 255, 255, 255],
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(4),
            rows_per_image: Some(1),
        },
        wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
    );
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("vrm.dummy_base_color.sampler"),
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        ..wgpu::SamplerDescriptor::default()
    });
    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("vrm.dummy_base_color.bg"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(&sampler),
            },
        ],
    });
    DummyBaseColorGpu {
        texture,
        sampler,
        bind_group,
    }
}

/// Shared dummy morph bind group for primitives without morph
/// targets. The bound storage buffer is a single `vec4` of
/// zeros; the bound meta has `target_count = 0` so the shader
/// never indexes into it.
fn build_dummy_morph_gpu(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    bgl: &wgpu::BindGroupLayout,
) -> DummyMorphGpu {
    let offsets_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("vrm.morph_offsets_dummy"),
        contents: bytemuck::bytes_of(&[0.0f32; 4]),
        usage: wgpu::BufferUsages::STORAGE,
    });
    let meta_buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("vrm.morph_meta_dummy"),
        size: PrimitiveMorphMeta::SIZE,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let zero_meta = PrimitiveMorphMeta::default();
    queue.write_buffer(&meta_buf, 0, bytemuck::bytes_of(&zero_meta));
    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("vrm.morph_bg_dummy"),
        layout: bgl,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: offsets_buf.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: meta_buf.as_entire_binding(),
            },
        ],
    });
    DummyMorphGpu {
        offsets_buf,
        meta_buf,
        bind_group,
    }
}

/// Build the per-model skin-matrix palette. For models
/// with a populated `Skeleton` the palette is the pre-baked
/// `bind_matrices` (i.e. `inverse_bind[i].inverse()`). For
/// models with no skin a one-element `Mat4::IDENTITY` palette
/// is uploaded and the default `MeshVertex` (`joints=[0,0,0,0]`,
/// `weights=[1,0,0,0]`) reduces the shader math to `pos`.
///
/// The buffer is allocated with `COPY_DST` so the runtime can
/// overwrite the palette every frame with
/// `current_joint_world[i] * inverse_bind[i]` via
/// [`VrmRenderer::update_skin_palette`].
fn build_skin_gpu(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    bgl: &wgpu::BindGroupLayout,
    model: &VrmModel,
) -> SkinGpu {
    let joint_count = model.joint_count();
    if joint_count == 0 {
        // Identity palette: one `Mat4::IDENTITY`. The WGSL
        // palette index `0` is the only one ever read because
        // `joints = [0, 0, 0, 0]`.
        let identity: [[f32; 4]; 4] = glam::Mat4::IDENTITY.to_cols_array_2d();
        let matrices_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("vrm.skin_palette_identity"),
            contents: bytemuck::bytes_of(&identity),
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("vrm.skin_bg_identity"),
            layout: bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: matrices_buf.as_entire_binding(),
            }],
        });
        return SkinGpu {
            matrices_buf,
            bind_group,
            joint_count: IDENTITY_SKIN_PALETTE_LEN as u32,
        };
    }
    let mut palette: Vec<[[f32; 4]; 4]> = Vec::with_capacity(joint_count);
    for _ in 0..joint_count {
        palette.push(glam::Mat4::IDENTITY.to_cols_array_2d());
    }
    let matrices_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("vrm.skin_palette"),
        contents: bytemuck::cast_slice(&palette),
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
    });
    // `create_buffer_init` uploads the contents at creation
    // time; `queue` is kept in the signature so the API
    // stays symmetric with the per-frame palette overwrite.
    let _ = queue;
    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("vrm.skin_bg"),
        layout: bgl,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: matrices_buf.as_entire_binding(),
        }],
    });
    SkinGpu {
        matrices_buf,
        bind_group,
        joint_count: joint_count as u32,
    }
}
