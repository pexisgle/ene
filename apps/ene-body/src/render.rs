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

use crate::vrm::{RenderMesh, RenderTexture};

const HIT_TEST_CELL_PIXELS: u32 = 4;
const VISIBLE_ALPHA_THRESHOLD: f32 = 0.001;

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

/// Coarse alpha-aware ownership mask for native pointer hit-testing.
///
/// Four-pixel cells keep per-frame CPU work bounded while following the
/// deformed mesh closely enough that transparent desktop space remains owned
/// by the underlying application. One-cell dilation avoids tiny ungrabbable
/// gaps around thin geometry and texture-filtered edges.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HitTestMask {
    width: u32,
    height: u32,
    columns: u32,
    rows: u32,
    cells: Vec<bool>,
    bounds: Option<[u32; 4]>,
}

impl HitTestMask {
    pub(crate) fn empty(width: u32, height: u32) -> Self {
        let width = width.max(1);
        let height = height.max(1);
        let columns = width.div_ceil(HIT_TEST_CELL_PIXELS);
        let rows = height.div_ceil(HIT_TEST_CELL_PIXELS);
        let len = usize::try_from(u64::from(columns) * u64::from(rows)).unwrap_or(0);
        Self {
            width,
            height,
            columns,
            rows,
            cells: vec![false; len],
            bounds: None,
        }
    }

    pub(crate) fn from_meshes(meshes: &[RenderMesh], width: u32, height: u32) -> Self {
        let (vertices, indices, draws) = vertices(meshes, width, height);
        let textures = meshes
            .iter()
            .filter_map(|mesh| mesh.texture.as_ref())
            .map(|texture| (texture.id, texture))
            .collect::<BTreeMap<_, _>>();
        Self::from_frame(&vertices, &indices, draws, &textures, width, height)
    }

    fn from_frame(
        vertices: &[Vertex],
        indices: &[u32],
        draws: Vec<DrawRange>,
        textures: &BTreeMap<u64, &RenderTexture>,
        width: u32,
        height: u32,
    ) -> Self {
        let mut mask = Self::empty(width, height);
        for draw in draws {
            let Ok(start) = usize::try_from(draw.indices.start) else {
                continue;
            };
            let Ok(end) = usize::try_from(draw.indices.end) else {
                continue;
            };
            let Some(draw_indices) = indices.get(start..end) else {
                continue;
            };
            let texture = draw.texture_id.and_then(|id| textures.get(&id).copied());
            let (triangles, _remainder) = draw_indices.as_chunks::<3>();
            for triangle in triangles {
                let Some(a) = vertex_at(vertices, triangle[0]) else {
                    continue;
                };
                let Some(b) = vertex_at(vertices, triangle[1]) else {
                    continue;
                };
                let Some(c) = vertex_at(vertices, triangle[2]) else {
                    continue;
                };
                mask.rasterize_triangle(a, b, c, texture);
            }
        }
        mask.dilate_once();
        mask
    }

    pub(crate) fn contains(&self, x: i32, y: i32) -> bool {
        let (Ok(x), Ok(y)) = (u32::try_from(x), u32::try_from(y)) else {
            return false;
        };
        if x >= self.width || y >= self.height {
            return false;
        }
        let column = x / HIT_TEST_CELL_PIXELS;
        let row = y / HIT_TEST_CELL_PIXELS;
        self.cell(column, row)
    }

    pub(crate) fn contains_resize_grip(&self, x: i32, y: i32, grip: u32) -> bool {
        if !self.contains(x, y) {
            return false;
        }
        let (Ok(x), Ok(y), Some([_, _, _, bottom])) =
            (u32::try_from(x), u32::try_from(y), self.bounds)
        else {
            return false;
        };
        let grip_top = bottom.saturating_sub(grip);
        if y < grip_top {
            return false;
        }

        // A character's rightmost and bottommost pixels commonly belong to
        // different limbs. Requiring their bounding-box corner to be opaque
        // can therefore leave no resize target at all. Anchor the grip to the
        // rightmost visible cell in the bottom band instead; the target stays
        // on rendered character pixels without claiming transparent desktop.
        let first_row = grip_top / HIT_TEST_CELL_PIXELS;
        let last_row = bottom
            .saturating_sub(1)
            .div_euclid(HIT_TEST_CELL_PIXELS)
            .min(self.rows.saturating_sub(1));
        let mut band_right = 0;
        for row in first_row..=last_row {
            for column in (0..self.columns).rev() {
                if self.cell(column, row) {
                    band_right =
                        band_right.max(((column + 1) * HIT_TEST_CELL_PIXELS).min(self.width));
                    break;
                }
            }
        }
        band_right != 0 && x >= band_right.saturating_sub(grip)
    }

    /// Returns horizontal runs of owned cells in physical window coordinates.
    ///
    /// Windows uses these rectangles to make transparent cells absent from the
    /// HWND region. Returning row runs instead of one rectangle per cell keeps
    /// the native region bounded without weakening the alpha-aware mask.
    pub(crate) fn opaque_rectangles(&self) -> Vec<[u32; 4]> {
        let mut rectangles = Vec::new();
        for row in 0..self.rows {
            let mut column = 0;
            while column < self.columns {
                if !self.cell(column, row) {
                    column += 1;
                    continue;
                }
                let start = column;
                while column < self.columns && self.cell(column, row) {
                    column += 1;
                }
                rectangles.push([
                    start * HIT_TEST_CELL_PIXELS,
                    row * HIT_TEST_CELL_PIXELS,
                    (column * HIT_TEST_CELL_PIXELS).min(self.width),
                    ((row + 1) * HIT_TEST_CELL_PIXELS).min(self.height),
                ]);
            }
        }
        rectangles
    }

    fn rasterize_triangle(
        &mut self,
        a: &Vertex,
        b: &Vertex,
        c: &Vertex,
        texture: Option<&RenderTexture>,
    ) {
        let points = [
            screen_point(a, self.width, self.height),
            screen_point(b, self.width, self.height),
            screen_point(c, self.width, self.height),
        ];
        let min_x = points
            .iter()
            .map(|point| point[0])
            .fold(f32::INFINITY, f32::min)
            .max(0.0);
        let max_x = points
            .iter()
            .map(|point| point[0])
            .fold(f32::NEG_INFINITY, f32::max)
            .min(self.width as f32);
        let min_y = points
            .iter()
            .map(|point| point[1])
            .fold(f32::INFINITY, f32::min)
            .max(0.0);
        let max_y = points
            .iter()
            .map(|point| point[1])
            .fold(f32::NEG_INFINITY, f32::max)
            .min(self.height as f32);
        if min_x >= max_x || min_y >= max_y {
            return;
        }
        let first_column = (min_x as u32 / HIT_TEST_CELL_PIXELS).min(self.columns - 1);
        let last_column = (max_x as u32 / HIT_TEST_CELL_PIXELS).min(self.columns - 1);
        let first_row = (min_y as u32 / HIT_TEST_CELL_PIXELS).min(self.rows - 1);
        let last_row = (max_y as u32 / HIT_TEST_CELL_PIXELS).min(self.rows - 1);
        for row in first_row..=last_row {
            for column in first_column..=last_column {
                let point = [
                    (column * HIT_TEST_CELL_PIXELS) as f32 + HIT_TEST_CELL_PIXELS as f32 * 0.5,
                    (row * HIT_TEST_CELL_PIXELS) as f32 + HIT_TEST_CELL_PIXELS as f32 * 0.5,
                ];
                let Some(weights) = barycentric(point, points) else {
                    continue;
                };
                if fragment_alpha(a, b, c, weights, texture) > VISIBLE_ALPHA_THRESHOLD {
                    self.mark(column, row);
                }
            }
        }
        // Geometry smaller than one mask cell may not contain a cell center.
        // Retain its visible vertices before dilation so it remains draggable.
        for (vertex, point) in [(a, points[0]), (b, points[1]), (c, points[2])] {
            if vertex_alpha(vertex, texture) > VISIBLE_ALPHA_THRESHOLD {
                let column =
                    ((point[0].max(0.0) as u32) / HIT_TEST_CELL_PIXELS).min(self.columns - 1);
                let row = ((point[1].max(0.0) as u32) / HIT_TEST_CELL_PIXELS).min(self.rows - 1);
                self.mark(column, row);
            }
        }
    }

    fn dilate_once(&mut self) {
        let original = self.cells.clone();
        for row in 0..self.rows {
            for column in 0..self.columns {
                let Some(index) = self.index(column, row) else {
                    continue;
                };
                if !original.get(index).copied().unwrap_or(false) {
                    continue;
                }
                for y in row.saturating_sub(1)..=(row + 1).min(self.rows - 1) {
                    for x in column.saturating_sub(1)..=(column + 1).min(self.columns - 1) {
                        self.mark(x, y);
                    }
                }
            }
        }
    }

    fn mark(&mut self, column: u32, row: u32) {
        let Some(index) = self.index(column, row) else {
            return;
        };
        let Some(cell) = self.cells.get_mut(index) else {
            return;
        };
        *cell = true;
        let left = column * HIT_TEST_CELL_PIXELS;
        let top = row * HIT_TEST_CELL_PIXELS;
        let right = ((column + 1) * HIT_TEST_CELL_PIXELS).min(self.width);
        let bottom = ((row + 1) * HIT_TEST_CELL_PIXELS).min(self.height);
        self.bounds = Some(match self.bounds {
            Some([old_left, old_top, old_right, old_bottom]) => [
                old_left.min(left),
                old_top.min(top),
                old_right.max(right),
                old_bottom.max(bottom),
            ],
            None => [left, top, right, bottom],
        });
    }

    fn cell(&self, column: u32, row: u32) -> bool {
        self.index(column, row)
            .and_then(|index| self.cells.get(index))
            .copied()
            .unwrap_or(false)
    }

    fn index(&self, column: u32, row: u32) -> Option<usize> {
        if column >= self.columns || row >= self.rows {
            return None;
        }
        usize::try_from(u64::from(row) * u64::from(self.columns) + u64::from(column)).ok()
    }
}

/// Pixel extent of the alpha-aware cells.
///
/// The Wayland overlay sets its input region from [`HitTestMask::opaque_rectangles`]
/// and does not need the raw dimensions; the Windows DWM hit-test path and
/// this module's unit tests do.
#[cfg(any(target_os = "windows", test))]
impl HitTestMask {
    pub(crate) fn width(&self) -> u32 {
        self.width
    }

    pub(crate) fn height(&self) -> u32 {
        self.height
    }
}

fn vertex_at(vertices: &[Vertex], index: u32) -> Option<&Vertex> {
    usize::try_from(index)
        .ok()
        .and_then(|index| vertices.get(index))
}

fn screen_point(vertex: &Vertex, width: u32, height: u32) -> [f32; 2] {
    [
        (vertex.position[0] * 0.5 + 0.5) * width as f32,
        (0.5 - vertex.position[1] * 0.5) * height as f32,
    ]
}

fn barycentric(point: [f32; 2], triangle: [[f32; 2]; 3]) -> Option<[f32; 3]> {
    let [a, b, c] = triangle;
    let denominator = (b[1] - c[1]) * (a[0] - c[0]) + (c[0] - b[0]) * (a[1] - c[1]);
    if denominator.abs() <= f32::EPSILON {
        return None;
    }
    let first =
        ((b[1] - c[1]) * (point[0] - c[0]) + (c[0] - b[0]) * (point[1] - c[1])) / denominator;
    let second =
        ((c[1] - a[1]) * (point[0] - c[0]) + (a[0] - c[0]) * (point[1] - c[1])) / denominator;
    let third = 1.0 - first - second;
    (first >= 0.0 && second >= 0.0 && third >= 0.0).then_some([first, second, third])
}

fn fragment_alpha(
    a: &Vertex,
    b: &Vertex,
    c: &Vertex,
    weights: [f32; 3],
    texture: Option<&RenderTexture>,
) -> f32 {
    let vertex_alpha = a.color[3] * weights[0] + b.color[3] * weights[1] + c.color[3] * weights[2];
    let uv = [
        a.uv[0] * weights[0] + b.uv[0] * weights[1] + c.uv[0] * weights[2],
        a.uv[1] * weights[0] + b.uv[1] * weights[1] + c.uv[1] * weights[2],
    ];
    vertex_alpha * texture_alpha(texture, uv)
}

fn vertex_alpha(vertex: &Vertex, texture: Option<&RenderTexture>) -> f32 {
    vertex.color[3] * texture_alpha(texture, vertex.uv)
}

fn texture_alpha(texture: Option<&RenderTexture>, uv: [f32; 2]) -> f32 {
    let Some(texture) = texture else {
        return 1.0;
    };
    if texture.width == 0 || texture.height == 0 {
        return 0.0;
    }
    let x = ((uv[0].rem_euclid(1.0) * texture.width as f32).floor() as u32).min(texture.width - 1);
    let y =
        ((uv[1].rem_euclid(1.0) * texture.height as f32).floor() as u32).min(texture.height - 1);
    let offset = u64::from(y)
        .checked_mul(u64::from(texture.width))
        .and_then(|row| row.checked_add(u64::from(x)))
        .and_then(|pixel| pixel.checked_mul(4))
        .and_then(|byte| byte.checked_add(3))
        .and_then(|offset| usize::try_from(offset).ok());
    offset
        .and_then(|offset| texture.rgba.get(offset))
        .map_or(0.0, |alpha| f32::from(*alpha) / 255.0)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn triangle_vertices() -> Vec<Vertex> {
        vec![
            Vertex {
                position: [-0.8, 0.8, 0.5],
                color: [1.0; 4],
                uv: [0.5, 0.5],
            },
            Vertex {
                position: [-0.8, -0.8, 0.5],
                color: [1.0; 4],
                uv: [0.5, 0.5],
            },
            Vertex {
                position: [0.2, 0.0, 0.5],
                color: [1.0; 4],
                uv: [0.5, 0.5],
            },
        ]
    }

    #[test]
    fn hit_test_mask_owns_visible_triangle_not_its_window() {
        let mask = HitTestMask::from_frame(
            &triangle_vertices(),
            &[0, 1, 2],
            vec![DrawRange {
                indices: 0..3,
                texture_id: None,
            }],
            &BTreeMap::new(),
            100,
            100,
        );
        assert!(mask.contains(25, 50));
        assert!(!mask.contains(90, 90));
        assert!(!mask.contains_resize_grip(90, 90, 16));
    }

    #[test]
    fn resize_grip_uses_bottom_band_when_right_and_bottom_extents_differ() {
        let mut mask = HitTestMask::empty(100, 100);
        mask.mark(24, 5);
        mask.mark(4, 24);

        assert!(mask.contains_resize_grip(18, 98, 16));
        assert!(!mask.contains_resize_grip(98, 22, 16));
        assert!(!mask.contains_resize_grip(40, 98, 16));
    }

    #[test]
    fn fully_transparent_texture_does_not_own_pointer_input() {
        let transparent = RenderTexture {
            id: 7,
            width: 1,
            height: 1,
            rgba: Arc::<[u8]>::from([255, 255, 255, 0]),
        };
        let textures = BTreeMap::from([(transparent.id, &transparent)]);
        let mask = HitTestMask::from_frame(
            &triangle_vertices(),
            &[0, 1, 2],
            vec![DrawRange {
                indices: 0..3,
                texture_id: Some(transparent.id),
            }],
            &textures,
            100,
            100,
        );
        assert!(!mask.contains(25, 50));
        assert!(mask.opaque_rectangles().is_empty());
    }

    #[test]
    fn opaque_rectangles_coalesce_horizontal_cells_and_clip_edges() {
        let mut mask = HitTestMask::empty(10, 6);
        mask.mark(0, 0);
        mask.mark(1, 0);
        mask.mark(2, 1);
        assert_eq!(mask.opaque_rectangles(), vec![[0, 0, 8, 4], [8, 4, 10, 6]]);
    }

    #[test]
    fn resized_empty_mask_has_no_owned_rectangles() {
        let mask = HitTestMask::empty(630, 960);
        assert_eq!(mask.width(), 630);
        assert_eq!(mask.height(), 960);
        assert!(mask.opaque_rectangles().is_empty());
    }
}
