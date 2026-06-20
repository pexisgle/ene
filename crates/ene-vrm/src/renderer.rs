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
//! - **PR4.4**: bind group `(3)` carries the morph-target data
//!   (storage + uniform). Primitives that define morph targets
//!   get a per-primitive storage buffer (the position
//!   displacements, already normalized by the loader) and a
//!   uniform [`PrimitiveMorphMeta`] that the renderer fills
//!   every frame from the model's global
//!   `BTreeMap<ExpressionName, f32>` weight map. Primitives
//!   without morph targets bind a shared dummy layout with
//!   `target_count = 0`; the shader's `if (target_count > 0u)`
//!   early-out makes the cost near zero. The slot index used
//!   to look up weights is the **per-primitive local** index
//!   (the position of the target in `PrimitiveMorphs::targets`)
//!   — not a global name-flattened index — so the shader's
//!   `weights[t / 4][t % 4]` lookup always matches the
//!   corresponding row in the offsets storage buffer.
//!
//! The full MToon shader (rim / matcap / outline / emission) is a
//! follow-up PR.
use wgpu::util::DeviceExt;

use crate::camera::{CameraUniform, ModelUniform, OrthographicCamera};
use crate::expression::{PrimitiveMorphMeta, PrimitiveMorphs};
use crate::model::{AlphaMode, VrmModel};

const SHADER_SOURCE: &str = include_str!("shaders/mtoon_skinned.wgsl");
const UNLIT_SHADER_SOURCE: &str = include_str!("shaders/unlit_skinned.wgsl");
const MTOON_SHADER_SOURCE: &str = include_str!("shaders/mtoon_full.wgsl");

/// PR4.5: number of skin-matrix palette slots to allocate when the
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
    #[allow(dead_code)]
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
    #[allow(dead_code)]
    target_count: u32,
    /// Cached copy of the primitive's vertex count. Same
    /// purpose as `target_count`.
    #[allow(dead_code)]
    vertex_count: u32,
}

/// Shared dummy morph bind group used by every primitive without
/// morph targets. Keeps the pipeline layout (which always
/// declares group `(3)`) consistent without paying for per-
/// primitive dummy storage allocations.
struct DummyMorphGpu {
    /// Single `vec4` storage entry. Never read by the shader
    /// because the bound `meta_buf` has `target_count = 0`.
    #[allow(dead_code)]
    offsets_buf: wgpu::Buffer,
    /// Meta uniform with `target_count = 0` and zero weights.
    #[allow(dead_code)]
    meta_buf: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
}

/// PR4.5: per-model skin-matrix palette. One `mat4x4<f32>` per
/// joint, uploaded once at construction time with the
/// pre-baked `bind_matrices` (i.e. `inverse_bind[i].inverse()`).
/// Phase 2 will overwrite the buffer every frame with
/// `current_joint_world[i] * bind_matrices[i]` to drive the
/// cursor look-at + two-bone IK.
struct SkinGpu {
    matrices_buf: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    /// Number of joints (i.e. the palette length). Used by
    /// callers that want to grow the storage when the loader
    /// finds more joints than expected (defensive — the loader
    /// always produces a fixed-size buffer).
    joint_count: u32,
}

/// PR4.15: per-primitive MToon uniform buffer (group 5).
struct MToonUniformGpu {
    bind_group: wgpu::BindGroup,
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
    /// Issue #19: unlit opaque pipeline. Same depth state as
    /// `pipeline_opaque` but uses the unlit shader (no lighting).
    pipeline_unlit_opaque: wgpu::RenderPipeline,
    /// Issue #19: unlit transparent pipeline. Same blend state
    /// as `pipeline_transparent` but uses the unlit shader.
    pipeline_unlit_transparent: wgpu::RenderPipeline,
    /// Dummy morph resources, bound for primitives that have no
    /// morph targets.
    dummy_morph: DummyMorphGpu,
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
    /// PR4.5: skin-matrix palette (group 4). Uploaded once
    /// with the model's `bind_matrices` (rest pose). Phase 2
    /// will rewrite this every frame to drive look-at
    /// rotations.
    skin: SkinGpu,
    /// PR4.15: MToon per-material uniform buffer (group 5).
    /// One buffer per primitive that has MToon; `None` for
    /// primitives that use the lite shader.
    mtoon_uniforms: Vec<Option<MToonUniformGpu>>,
    /// PR4.15: MToon opaque pipeline.
    pipeline_mtoon_opaque: wgpu::RenderPipeline,
    /// PR4.15: MToon transparent pipeline.
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

        // PR4.4: group `(3)` — morph targets. The layout is
        // shared by every per-primitive bind group and the
        // dummy group.
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

        // PR4.5: group `(4)` — skin-matrix palette. The
        // renderer uploads one `mat4x4<f32>` per joint, or a
        // single `Mat4::IDENTITY` for models without a skin
        // (the default MeshVertex falls back to
        // `joints=[0,0,0,0]` + `weights=[1,0,0,0]` so the
        // skinned shader reduces to `pos`).
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

        // PR4.15: group `(5)` — MToon per-material uniform.
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

        // PR4.15: group `(6)` — MToon textures (14 bindings: 7 tex + 7 smp).
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

        // PR4.15: MToon pipeline layout (7 bind groups).
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

        // PR4.15: MToon full shader.
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

        // Issue #19: unlit opaque pipeline. Same layout and
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

        // Issue #19: unlit transparent pipeline. Same blend
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

        // PR4.15: MToon opaque pipeline. Uses the full MToon
        // shader with 7 bind groups.
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

        // PR4.15: MToon transparent pipeline.
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

        // PR4.4: build the per-primitive morph GPU resources.
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

        // Allocate a single dummy bind group used by every
        // primitive without morph targets. The shader never
        // reads from the storage buffer because the bound meta
        // has `target_count = 0`.
        let dummy_morph = build_dummy_morph_gpu(device, queue, &morph_bgl);

        // PR4.5: build the skin-matrix palette. For models with
        // a populated `Skeleton` we upload every
        // `bind_matrices[i]` once (rest pose). For models
        // without a skin we upload a single `Mat4::IDENTITY`
        // and the default MeshVertex (`joints = [0,0,0,0]`,
        // `weights = [1,0,0,0]`) reduces the shader math to
        // `pos`.
        let skin = build_skin_gpu(device, queue, &skin_bgl, model);

        // PR4.15: build per-primitive MToon uniform buffers.
        // Primitives without MToon get `None` slots.
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
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs_main"),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: fmt,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: Default::default(),
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
            morph_gpu,
            meta_scratch: PrimitiveMorphMeta::default(),
            skin,
            mtoon_uniforms,
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
    /// clear color.
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

        // PR4.4: each morph-capable primitive is rendered with
        // its own pre-built bind group (group 3). The slot index
        // used to look up weights is the **per-primitive local**
        // index — see `upload_morph_meta` for the rationale.

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

        rp.set_pipeline(&self.pipeline_opaque);
        rp.set_bind_group(0, &self.camera_bind_group, &[]);
        rp.set_bind_group(1, &self.model_bind_group, &[]);
        rp.set_bind_group(4, &self.skin.bind_group, &[]);

        // Build a flat draw list with alpha mode and unlit flag
        // so we can sort opaque/mask before transparent and
        // route unlit primitives to the unlit pipeline.
        #[derive(Copy, Clone)]
        struct DrawItem {
            linear_index: usize,
            alpha_mode: AlphaMode,
            unlit: bool,
        }
        let mut draw_items: Vec<DrawItem> = Vec::new();
        {
            let mut idx: usize = 0;
            for mesh in &model.meshes {
                for prim in &mesh.primitives {
                    draw_items.push(DrawItem {
                        linear_index: idx,
                        alpha_mode: prim.alpha_mode,
                        unlit: prim.unlit,
                    });
                    idx += 1;
                }
            }
        }
        draw_items.sort_by_key(|d| d.alpha_mode.render_phase());

        // We need access to the vertex/index buffers by linear_index.
        // Build a flat reference array once.
        let all_prims: Vec<&_> = model
            .meshes
            .iter()
            .flat_map(|m| m.primitives.iter())
            .collect();

        // Phase 1: opaque + mask (depth write on, no blending).
        for item in draw_items
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

        // Phase 2: transparent (depth write off, premultiplied
        // alpha blending). Drawn in declaration order (which is
        // roughly back-to-front for most humanoid VRM models);
        // a proper view-Z depth sort is a follow-up PR.
        for item in draw_items
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
        let pipeline = match &self.pipeline_mask {
            Some(p) => p,
            None => return,
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

        let all_prims: Vec<&_> = model
            .meshes
            .iter()
            .flat_map(|m| m.primitives.iter())
            .collect();

        for (idx, prim) in all_prims.into_iter().enumerate() {
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

    /// Bind the per-primitive resources and issue a single draw.
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
        // PR4.15: bind MToon uniform + textures if present.
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

    /// PR4.16: overwrite the skin-palette storage buffer with
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
    /// not call this on every frame for an unsninned
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

    /// PR4.16: the joint count of the renderer's skin
    /// palette. Zero for models built with the identity
    /// one-element palette (no skin).
    #[allow(dead_code)]
    pub fn skin_joint_count(&self) -> u32 {
        self.skin.joint_count
    }
}

/// Build a one-shot morph bind group for a single primitive that
/// has at least one morph target. The storage buffer is uploaded
/// once with the loader's normalized displacement data; the meta
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

/// PR4.5: build the per-model skin-matrix palette. For models
/// with a populated `Skeleton` the palette is the pre-baked
/// `bind_matrices` (i.e. `inverse_bind[i].inverse()`). For
/// models with no skin a one-element `Mat4::IDENTITY` palette
/// is uploaded and the default `MeshVertex` (`joints=[0,0,0,0]`,
/// `weights=[1,0,0,0]`) reduces the shader math to `pos`.
///
/// The buffer is allocated with `COPY_DST` so Phase 2 can
/// overwrite the palette every frame with
/// `current_joint_world[i] * bind_matrices[i]` to drive the
/// cursor look-at + two-bone IK. Phase 1 (this PR) uploads once
/// at construction and never touches the buffer again.
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
    // Skinned palette: one `mat4x4<f32>` per joint, initialized to identity at rest pose.
    let mut palette: Vec<[[f32; 4]; 4]> = Vec::with_capacity(joint_count);
    for _ in 0..joint_count {
        palette.push(glam::Mat4::IDENTITY.to_cols_array_2d());
    }
    let matrices_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("vrm.skin_palette"),
        contents: bytemuck::cast_slice(&palette),
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
    });
    // Make sure the storage is on the GPU before the first
    // draw (create_buffer_init with a non-mapped buffer does
    // not need an explicit write — the contents are placed at
    // creation time, but we issue a no-op write to keep the
    // API symmetric with future Phase 2 per-frame updates).
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
