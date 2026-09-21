//! wgpu surface ownership for this process only.
//!
//! Host and `ene-desktop` must not open a GPU device. A successful adapter and
//! surface creation are still not real-compositor acceptance; display evidence
//! comes from Wayland presentation feedback or correlated Windows timing.

use raw_window_handle::{RawDisplayHandle, RawWindowHandle};
use std::collections::BTreeMap;
use std::ops::Range;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use wgpu::util::DeviceExt as _;

use crate::vrm::RenderMesh;

/// A transparent real-surface renderer. VRM deformation stays in
/// `vrm-runtime`; this deliberately small unlit path is the design-approved
/// fallback while MToon is not implemented.
pub struct SurfaceRenderer {
    _instance: wgpu::Instance,
    surface: wgpu::Surface<'static>,
    adapter: wgpu::Adapter,
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipeline: wgpu::RenderPipeline,
    texture_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    fallback_texture: wgpu::BindGroup,
    textures: BTreeMap<u64, wgpu::BindGroup>,
    depth_texture: wgpu::Texture,
    depth_view: wgpu::TextureView,
    config: wgpu::SurfaceConfiguration,
    lost: Arc<AtomicBool>,
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Vertex {
    position: [f32; 3],
    color: [f32; 4],
    uv: [f32; 2],
}

struct DrawRange {
    indices: Range<u32>,
    texture_id: Option<u64>,
}

/// Surface rendering failure. The platform thread reports this to the parent
/// and exits; it never takes the desktop or Host with it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenderFailure {
    Surface,
    Adapter,
    Device,
    DeviceLost,
    OutOfMemory,
}

/// Whether this call actually submitted a surface texture for presentation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenderOutcome {
    Presented,
    Skipped,
}

impl SurfaceRenderer {
    /// Creates wgpu against an already-created native overlay surface.
    ///
    /// # Safety
    ///
    /// The caller must keep both native display and window objects alive until
    /// this renderer is dropped, and must use the handles on their owning
    /// platform thread.
    pub async unsafe fn new(
        display: RawDisplayHandle,
        window: RawWindowHandle,
        width: u32,
        height: u32,
    ) -> Result<Self, RenderFailure> {
        // HWND swap chains expose only opaque alpha on DX12. The native
        // Windows overlay needs a DirectComposition visual for per-pixel alpha.
        #[cfg(target_os = "windows")]
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::DX12,
            backend_options: wgpu::BackendOptions {
                dx12: wgpu::Dx12BackendOptions {
                    presentation_system: wgpu::wgt::Dx12SwapchainKind::DxgiFromVisual,
                    ..Default::default()
                },
                ..Default::default()
            },
            ..Default::default()
        });
        #[cfg(not(target_os = "windows"))]
        let instance = wgpu::Instance::default();
        // SAFETY: upheld by this method's caller contract. Platform backends
        // own the native objects and renderer together on one thread.
        let surface = unsafe {
            instance.create_surface_unsafe(wgpu::SurfaceTargetUnsafe::RawHandle {
                raw_display_handle: display,
                raw_window_handle: window,
            })
        }
        .map_err(|_| RenderFailure::Surface)?;
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::LowPower,
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
            })
            .await
            .map_err(|_| RenderFailure::Adapter)?;
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("ene-body surface"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::downlevel_defaults(),
                experimental_features: wgpu::ExperimentalFeatures::disabled(),
                memory_hints: wgpu::MemoryHints::MemoryUsage,
                trace: wgpu::Trace::Off,
            })
            .await
            .map_err(|_| RenderFailure::Device)?;
        let lost = Arc::new(AtomicBool::new(false));
        let lost_callback = Arc::clone(&lost);
        device.set_device_lost_callback(move |_reason, _message| {
            lost_callback.store(true, Ordering::Release);
        });
        let capabilities = surface.get_capabilities(&adapter);
        let format = capabilities
            .formats
            .iter()
            .copied()
            .find(wgpu::TextureFormat::is_srgb)
            .or_else(|| capabilities.formats.first().copied())
            .ok_or(RenderFailure::Surface)?;
        let alpha_mode = if capabilities
            .alpha_modes
            .contains(&wgpu::CompositeAlphaMode::PreMultiplied)
        {
            wgpu::CompositeAlphaMode::PreMultiplied
        } else if capabilities
            .alpha_modes
            .contains(&wgpu::CompositeAlphaMode::PostMultiplied)
        {
            wgpu::CompositeAlphaMode::PostMultiplied
        } else {
            return Err(RenderFailure::Surface);
        };
        let blend = if alpha_mode == wgpu::CompositeAlphaMode::PreMultiplied {
            wgpu::BlendState::ALPHA_BLENDING
        } else {
            wgpu::BlendState {
                color: wgpu::BlendComponent {
                    src_factor: wgpu::BlendFactor::One,
                    dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                    operation: wgpu::BlendOperation::Add,
                },
                alpha: wgpu::BlendComponent::OVER,
            }
        };
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: width.max(1),
            height: height.max(1),
            present_mode: wgpu::PresentMode::Fifo,
            desired_maximum_frame_latency: 2,
            alpha_mode,
            view_formats: vec![format],
        };
        surface.configure(&device, &config);
        let texture_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("ene-body base color texture"),
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
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("ene-body base color sampler"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let fallback_texture = create_texture_binding(
            &device,
            &queue,
            &texture_layout,
            &sampler,
            "ene-body white texture",
            (1, 1),
            &[255, 255, 255, 255],
        );
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("ene-body unlit fallback"),
            source: wgpu::ShaderSource::Wgsl(
                r#"
struct VertexIn {
    @location(0) position: vec3<f32>,
    @location(1) color: vec4<f32>,
    @location(2) uv: vec2<f32>,
};
struct VertexOut {
    @builtin(position) position: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) uv: vec2<f32>,
};
@group(0) @binding(0) var base_color_texture: texture_2d<f32>;
@group(0) @binding(1) var base_color_sampler: sampler;
@vertex fn vertex_main(input: VertexIn) -> VertexOut {
    var out: VertexOut;
    out.position = vec4<f32>(input.position, 1.0);
    out.color = input.color;
    out.uv = input.uv;
    return out;
}
@fragment fn fragment_main(input: VertexOut) -> @location(0) vec4<f32> {
    let color = input.color * textureSample(base_color_texture, base_color_sampler, input.uv);
    if color.a <= 0.001 {
        discard;
    }
    return color;
}
"#
                .into(),
            ),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("ene-body unlit fallback"),
            bind_group_layouts: &[&texture_layout],
            push_constant_ranges: &[],
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("ene-body unlit fallback"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vertex_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<Vertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &[
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x3,
                            offset: 0,
                            shader_location: 0,
                        },
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x4,
                            offset: 12,
                            shader_location: 1,
                        },
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x2,
                            offset: 28,
                            shader_location: 2,
                        },
                    ],
                }],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth24Plus,
                depth_write_enabled: true,
                depth_compare: wgpu::CompareFunction::LessEqual,
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fragment_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(blend),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview: None,
            cache: None,
        });
        let (depth_texture, depth_view) = create_depth(&device, config.width, config.height);
        Ok(Self {
            _instance: instance,
            surface,
            adapter,
            device,
            queue,
            pipeline,
            texture_layout,
            sampler,
            fallback_texture,
            textures: BTreeMap::new(),
            depth_texture,
            depth_view,
            config,
            lost,
        })
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        if (self.config.width, self.config.height) == (width.max(1), height.max(1)) {
            return;
        }
        self.config.width = width.max(1);
        self.config.height = height.max(1);
        self.surface.configure(&self.device, &self.config);
        (self.depth_texture, self.depth_view) =
            create_depth(&self.device, self.config.width, self.config.height);
    }

    /// Submits and presents one frame. Success means submitted to the
    /// compositor, not displayed; presentation feedback remains the FPS owner.
    pub fn render(&mut self, meshes: &[RenderMesh]) -> Result<RenderOutcome, RenderFailure> {
        if self.lost.load(Ordering::Acquire) {
            return Err(RenderFailure::DeviceLost);
        }
        for texture in meshes.iter().filter_map(|mesh| mesh.texture.as_ref()) {
            if !self.textures.contains_key(&texture.id) {
                let binding = create_texture_binding(
                    &self.device,
                    &self.queue,
                    &self.texture_layout,
                    &self.sampler,
                    "ene-body avatar texture",
                    (texture.width, texture.height),
                    &texture.rgba,
                );
                self.textures.insert(texture.id, binding);
            }
        }
        let (vertices, indices, draws) = vertices(meshes, self.config.width, self.config.height);
        let frame = match self.surface.get_current_texture() {
            Ok(frame) => frame,
            Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
                self.surface.configure(&self.device, &self.config);
                return Ok(RenderOutcome::Skipped);
            }
            Err(wgpu::SurfaceError::Timeout) => return Ok(RenderOutcome::Skipped),
            Err(wgpu::SurfaceError::OutOfMemory) => return Err(RenderFailure::OutOfMemory),
            Err(wgpu::SurfaceError::Other) => return Err(RenderFailure::Surface),
        };
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let vertex_buffer = (!vertices.is_empty()).then(|| {
            self.device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("ene-body frame vertices"),
                    contents: bytemuck::cast_slice(&vertices),
                    usage: wgpu::BufferUsages::VERTEX,
                })
        });
        let index_buffer = (!indices.is_empty()).then(|| {
            self.device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("ene-body frame indices"),
                    contents: bytemuck::cast_slice(&indices),
                    usage: wgpu::BufferUsages::INDEX,
                })
        });
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("ene-body frame"),
            });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("ene-body transparent pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Discard,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            if let (Some(vertex_buffer), Some(index_buffer)) = (&vertex_buffer, &index_buffer) {
                pass.set_pipeline(&self.pipeline);
                pass.set_vertex_buffer(0, vertex_buffer.slice(..));
                pass.set_index_buffer(index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                for draw in draws {
                    let texture = draw
                        .texture_id
                        .and_then(|id| self.textures.get(&id))
                        .unwrap_or(&self.fallback_texture);
                    pass.set_bind_group(0, texture, &[]);
                    pass.draw_indexed(draw.indices, 0, 0..1);
                }
            }
        }
        self.queue.submit([encoder.finish()]);
        frame.present();
        Ok(RenderOutcome::Presented)
    }

    #[must_use]
    pub fn adapter_info(&self) -> wgpu::AdapterInfo {
        self.adapter.get_info()
    }
}

fn vertices(
    meshes: &[RenderMesh],
    viewport_width: u32,
    viewport_height: u32,
) -> (Vec<Vertex>, Vec<u32>, Vec<DrawRange>) {
    let mut min = [f32::INFINITY; 3];
    let mut max = [f32::NEG_INFINITY; 3];
    for position in meshes.iter().flat_map(|mesh| &mesh.mesh.positions) {
        for axis in 0..3 {
            min[axis] = min[axis].min(position[axis]);
            max[axis] = max[axis].max(position[axis]);
        }
    }
    if !min[0].is_finite() {
        return (Vec::new(), Vec::new(), Vec::new());
    }
    let center = [(min[0] + max[0]) * 0.5, (min[1] + max[1]) * 0.5];
    let model_width = (max[0] - min[0]).max(0.001);
    let model_height = (max[1] - min[1]).max(0.001);
    let viewport_width = viewport_width.max(1) as f32;
    let viewport_height = viewport_height.max(1) as f32;
    let pixels_per_unit =
        (viewport_width * 0.9 / model_width).min(viewport_height * 0.9 / model_height);
    let depth_range = (max[2] - min[2]).max(0.001);
    let mut vertices = Vec::new();
    let mut indices = Vec::new();
    let mut draws = Vec::new();
    for mesh in meshes {
        if mesh.mesh.topology != vrm_runtime::PrimitiveTopology::Triangles {
            continue;
        }
        let base = u32::try_from(vertices.len()).unwrap_or(u32::MAX);
        vertices.extend(
            mesh.mesh
                .positions
                .iter()
                .enumerate()
                .map(|(index, position)| {
                    let vertex_color = mesh.mesh.colors.get(index).copied().unwrap_or([1.0; 4]);
                    Vertex {
                        position: [
                            (position[0] - center[0]) * pixels_per_unit * 2.0 / viewport_width,
                            (position[1] - center[1]) * pixels_per_unit * 2.0 / viewport_height,
                            0.05 + 0.9 * (max[2] - position[2]) / depth_range,
                        ],
                        color: [
                            vertex_color[0] * mesh.base_color[0],
                            vertex_color[1] * mesh.base_color[1],
                            vertex_color[2] * mesh.base_color[2],
                            vertex_color[3] * mesh.base_color[3],
                        ],
                        uv: mesh.texcoords.get(index).copied().unwrap_or([0.0, 0.0]),
                    }
                }),
        );
        let index_start = u32::try_from(indices.len()).unwrap_or(u32::MAX);
        if mesh.mesh.indices.is_empty() {
            indices.extend((0..mesh.mesh.positions.len()).filter_map(|index| {
                u32::try_from(index)
                    .ok()
                    .and_then(|index| base.checked_add(index))
            }));
        } else {
            indices.extend(
                mesh.mesh
                    .indices
                    .iter()
                    .filter_map(|index| base.checked_add(*index)),
            );
        }
        let index_end = u32::try_from(indices.len()).unwrap_or(u32::MAX);
        draws.push(DrawRange {
            indices: index_start..index_end,
            texture_id: (!mesh.texcoords.is_empty())
                .then(|| mesh.texture.as_ref().map(|texture| texture.id))
                .flatten(),
        });
    }
    (vertices, indices, draws)
}

fn create_texture_binding(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    layout: &wgpu::BindGroupLayout,
    sampler: &wgpu::Sampler,
    label: &str,
    dimensions: (u32, u32),
    rgba: &[u8],
) -> wgpu::BindGroup {
    let (width, height) = dimensions;
    let size = wgpu::Extent3d {
        width: width.max(1),
        height: height.max(1),
        depth_or_array_layers: 1,
    };
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size,
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
        rgba,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(4 * width.max(1)),
            rows_per_image: Some(height.max(1)),
        },
        size,
    );
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some(label),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
        ],
    })
}

fn create_depth(
    device: &wgpu::Device,
    width: u32,
    height: u32,
) -> (wgpu::Texture, wgpu::TextureView) {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("ene-body depth"),
        size: wgpu::Extent3d {
            width: width.max(1),
            height: height.max(1),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Depth24Plus,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    (texture, view)
}
