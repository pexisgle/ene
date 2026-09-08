//! Raycast / collider debug renderer.
//!
//! A minimal wgpu line-list renderer for the per-bone
//! sphere colliders. The runtime collects one `DebugLine` per
//! collider sphere edge and one `DebugLine` per hit-point
//! cross segment, then calls [`DebugRenderer::render`] after
//! the main VRM pass with the same camera and the same
//! `Depth32Float` depth attachment (loaded, not cleared).
//! The pipeline's depth test is `CompareFunction::Always`, so
//! the wires draw over the model regardless of depth.
//!
//! Performance budget: a 50-bone character with 16 longitudes
//! and 8 latitudes per sphere is 50 * (16 + 8) = 1,200 lines
//! = 2,400 vertices. The default capacity of 8,192 lines
//! (16,384 vertices) gives ~6× headroom for the cross + any
//! future visualisations.
use crate::camera::CameraUniform;

const SHADER_SOURCE: &str = include_str!("shaders/debug_line.wgsl");

/// Default vertex buffer capacity in **lines** (each line is
/// two vertices). 8,192 lines = 16,384 vertices is well
/// above the 50-bone sphere budget and small enough to keep
/// the per-frame upload cheap.
///
/// Hidden from the "Supported API" docs — an internal GPU vertex-buffer sizing constant;
/// `ene-desktop` only interacts with [`DebugRenderer`] as an opaque handle.
#[doc(hidden)]
pub const DEFAULT_LINE_CAPACITY: usize = 8_192;

/// Higher values smooth the silhouette at the cost of more vertices.
pub const SPHERE_LONGITUDES: usize = 16;

/// Not counting the equator: the equator is always drawn (it's a latitude
/// too), so the total latitude line count is `SPHERE_LATITUDES + 1`.
pub const SPHERE_LATITUDES: usize = 8;

/// Half-edge length (in world units) of the cross drawn at
/// the raycast hit point. 0.05 m is small enough to read as
/// a marker but large enough to be picked out at 640×480.
pub const CROSS_HALF_EXTENT: f32 = 0.05;

/// Hidden from the "Supported API" docs — an internal GPU vertex format; hosts build
/// [`DebugLine`]s and never touch this type directly.
#[doc(hidden)]
#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct DebugVertex {
    /// World-space position of this endpoint of the line segment.
    pub pos: [f32; 3],
    /// Linear-space RGBA, premultiplied-alpha composited on the way out.
    pub color: [f32; 4],
}

impl DebugVertex {
    const ATTRIBUTES: [wgpu::VertexAttribute; 2] = wgpu::vertex_attr_array![
        0 => Float32x3,
        1 => Float32x4,
    ];

    pub(crate) const LAYOUT: wgpu::VertexBufferLayout<'static> = wgpu::VertexBufferLayout {
        array_stride: std::mem::size_of::<Self>() as wgpu::BufferAddress,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &Self::ATTRIBUTES,
    };
}

#[derive(Debug, Clone, Copy)]
pub struct DebugLine {
    /// First endpoint, in world space.
    pub a: glam::Vec3,
    /// Second endpoint, in world space.
    pub b: glam::Vec3,
    /// Linear-space RGBA color applied to both endpoints.
    pub color: glam::Vec4,
}

impl DebugLine {
    fn vertices(self) -> [DebugVertex; 2] {
        [
            DebugVertex {
                pos: [self.a.x, self.a.y, self.a.z],
                color: [self.color.x, self.color.y, self.color.z, self.color.w],
            },
            DebugVertex {
                pos: [self.b.x, self.b.y, self.b.z],
                color: [self.color.x, self.color.y, self.color.z, self.color.w],
            },
        ]
    }
}

/// The sphere is composed of `SPHERE_LONGITUDES` meridians plus
/// `SPHERE_LATITUDES + 1` latitude rings, each broken into
/// `SPHERE_LONGITUDES` segments. All lines share `color`.
pub fn sphere_wireframe_lines_into(
    center: glam::Vec3,
    radius: f32,
    color: glam::Vec4,
    out: &mut Vec<DebugLine>,
) {
    out.reserve(SPHERE_LONGITUDES * SPHERE_LONGITUDES + (SPHERE_LATITUDES + 1) * SPHERE_LONGITUDES);

    // Meridians: `SPHERE_LONGITUDES` half-circles, each
    // made of `SPHERE_LONGITUDES` straight segments. The
    // pole is a single shared point so the wireframe
    // doesn't degenerate at the top / bottom of the
    // sphere.
    for i in 0..SPHERE_LONGITUDES {
        let theta = (i as f32 / SPHERE_LONGITUDES as f32) * std::f32::consts::TAU;
        let (sin_t, cos_t) = theta.sin_cos();
        for k in 0..SPHERE_LONGITUDES {
            let phi0 = (k as f32 / SPHERE_LONGITUDES as f32) * std::f32::consts::PI;
            let phi1 = ((k + 1) as f32 / SPHERE_LONGITUDES as f32) * std::f32::consts::PI;
            let a = center
                + glam::Vec3::new(
                    radius * phi0.sin() * cos_t,
                    -radius * phi0.cos(),
                    radius * phi0.sin() * sin_t,
                );
            let b = center
                + glam::Vec3::new(
                    radius * phi1.sin() * cos_t,
                    -radius * phi1.cos(),
                    radius * phi1.sin() * sin_t,
                );
            out.push(DebugLine { a, b, color });
        }
    }

    for j in 0..=SPHERE_LATITUDES {
        let y = (2.0 * radius).mul_add(j as f32 / SPHERE_LATITUDES as f32, -radius);
        let ring_r = radius.mul_add(radius, -(y * y)).max(0.0).sqrt();
        for i in 0..SPHERE_LONGITUDES {
            let t0 = (i as f32 / SPHERE_LONGITUDES as f32) * std::f32::consts::TAU;
            let t1 = ((i + 1) as f32 / SPHERE_LONGITUDES as f32) * std::f32::consts::TAU;
            let a = center + glam::Vec3::new(ring_r * t0.cos(), y, ring_r * t0.sin());
            let b = center + glam::Vec3::new(ring_r * t1.cos(), y, ring_r * t1.sin());
            out.push(DebugLine { a, b, color });
        }
    }
}

/// The capsule is a cylinder of `half_height` capped by two
/// hemispheres of `radius`. Wireframe elements:
///
/// - top cap (a circle of `radius` at y = +`half_height`),
/// - bottom cap (a circle of `radius` at y = -`half_height`),
/// - `SPHERE_LONGITUDES` meridians running from the
///   bottom cap through the cylindrical section to the
///   top cap (each meridian is a single straight line
///   through the cylinder plus two arcs over the
///   hemispheres).
///
/// `orientation` rotates the capsule's local +Y to the
/// world direction the bone's "toward-child" axis points
/// in; `center` is the bone's world position. The function
/// is intentionally a thin wrapper over the sphere
/// wireframe helper — most of the geometry is just two
/// circles plus a handful of meridians.
pub fn capsule_wireframe_lines_into(
    center: glam::Vec3,
    half_height: f32,
    radius: f32,
    orientation: glam::Quat,
    color: glam::Vec4,
    out: &mut Vec<DebugLine>,
) {
    // The capsule's local +Y direction in world space:
    // the bone's "toward-child" axis (or the bone's local
    // +Y for the trunk).
    let y_axis = orientation * glam::Vec3::Y;
    let x_axis = orientation * glam::Vec3::X;
    let z_axis = orientation * glam::Vec3::Z;
    let top = center + y_axis * half_height;
    let bottom = center - y_axis * half_height;

    // The caps lie in the plane perpendicular to the capsule's +Y axis.
    for i in 0..SPHERE_LONGITUDES {
        let t0 = (i as f32 / SPHERE_LONGITUDES as f32) * std::f32::consts::TAU;
        let t1 = ((i + 1) as f32 / SPHERE_LONGITUDES as f32) * std::f32::consts::TAU;
        let a_top = top + (x_axis * t0.cos() + z_axis * t0.sin()) * radius;
        let b_top = top + (x_axis * t1.cos() + z_axis * t1.sin()) * radius;
        out.push(DebugLine {
            a: a_top,
            b: b_top,
            color,
        });
        let a_bot = bottom + (x_axis * t0.cos() + z_axis * t0.sin()) * radius;
        let b_bot = bottom + (x_axis * t1.cos() + z_axis * t1.sin()) * radius;
        out.push(DebugLine {
            a: a_bot,
            b: b_bot,
            color,
        });
    }

    // `SPHERE_LONGITUDES` meridians: each is a straight
    // line from the bottom cap, along the side, to the
    // top cap. The hemisphere arcs are not drawn — the
    // two caps and the side lines are enough to read the
    // shape in the debug overlay, and adding hemisphere
    // arcs would double the line count for marginal gain.
    for i in 0..SPHERE_LONGITUDES {
        let t = (i as f32 / SPHERE_LONGITUDES as f32) * std::f32::consts::TAU;
        let offset = (x_axis * t.cos() + z_axis * t.sin()) * radius;
        out.push(DebugLine {
            a: bottom + offset,
            b: top + offset,
            color,
        });
    }
}

/// Used to mark the raycast hit point so the precise impact location is
/// obvious.
pub fn cross_lines(
    center: glam::Vec3,
    half_extent: f32,
    color: glam::Vec4,
    out: &mut Vec<DebugLine>,
) {
    let x = glam::Vec3::X;
    let y = glam::Vec3::Y;
    let z = glam::Vec3::Z;
    out.push(DebugLine {
        a: center - x * half_extent,
        b: center + x * half_extent,
        color,
    });
    out.push(DebugLine {
        a: center - y * half_extent,
        b: center + y * half_extent,
        color,
    });
    out.push(DebugLine {
        a: center - z * half_extent,
        b: center + z * half_extent,
        color,
    });
}

/// Line-list wgpu renderer for debug overlays.
///
/// Owns the pipeline, the camera uniform buffer, and the
/// growing vertex buffer. Push lines with [`Self::push_line`]
/// (or the [`Self::push_sphere_wireframe`] /
///
/// [`Self::push_cross`] helpers), then call
/// [`Self::render`] once per frame to draw the accumulated
/// lines.
pub struct DebugRenderer {
    pipeline: wgpu::RenderPipeline,
    camera_buf: wgpu::Buffer,
    camera_bind_group: wgpu::BindGroup,
    vertex_buf: wgpu::Buffer,
    /// Maximum number of lines (pairs of vertices) the
    /// `vertex_buf` can hold. Doubled internally when the
    /// caller pushes more lines than fit.
    capacity_lines: usize,
    /// Per-frame line accumulator. Cleared on
    /// [`Self::render`].
    lines: Vec<DebugLine>,
}

impl DebugRenderer {
    /// The depth format is fixed at `Depth32Float` to match the main VRM
    /// pass; mixing depth formats would cause the GPU to reject the depth
    /// attachment.
    #[must_use]
    pub fn new(device: &wgpu::Device, surface_format: wgpu::TextureFormat) -> Self {
        let camera_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("debug.camera_buf"),
            size: std::mem::size_of::<CameraUniform>() as wgpu::BufferAddress,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let camera_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("debug.camera_bgl"),
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
            label: Some("debug.camera_bg"),
            layout: &camera_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: camera_buf.as_entire_binding(),
            }],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("debug.pipeline_layout"),
            bind_group_layouts: &[Some(&camera_bgl)],
            immediate_size: 0,
        });

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("debug.shader"),
            source: wgpu::ShaderSource::Wgsl(SHADER_SOURCE.into()),
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("debug.pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[DebugVertex::LAYOUT],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::LineList,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::Always),
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
                    // Premultiplied alpha so the overlay
                    // composites over the model's pixels
                    // without a stark color shift.
                    blend: Some(wgpu::BlendState {
                        color: wgpu::BlendComponent {
                            src_factor: wgpu::BlendFactor::One,
                            dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                            operation: wgpu::BlendOperation::Add,
                        },
                        alpha: wgpu::BlendComponent {
                            src_factor: wgpu::BlendFactor::One,
                            dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                            operation: wgpu::BlendOperation::Add,
                        },
                    }),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });

        let vertex_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("debug.vertex_buf"),
            size: (DEFAULT_LINE_CAPACITY * 2 * std::mem::size_of::<DebugVertex>())
                as wgpu::BufferAddress,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Self {
            pipeline,
            camera_buf,
            camera_bind_group,
            vertex_buf,
            capacity_lines: DEFAULT_LINE_CAPACITY,
            lines: Vec::new(),
        }
    }

    pub fn clear(&mut self) {
        self.lines.clear();
    }

    pub const fn line_count(&self) -> usize {
        self.lines.len()
    }

    pub fn push_line(&mut self, line: DebugLine) {
        self.lines.push(line);
    }

    pub fn push_sphere_wireframe(&mut self, center: glam::Vec3, radius: f32, color: glam::Vec4) {
        sphere_wireframe_lines_into(center, radius, color, &mut self.lines);
    }

    /// `orientation` rotates the capsule's local +Y to the world direction
    /// the bone's "toward-child" axis points in (identity for trunk bones).
    pub fn push_capsule_wireframe(
        &mut self,
        center: glam::Vec3,
        half_height: f32,
        radius: f32,
        orientation: glam::Quat,
        color: glam::Vec4,
    ) {
        capsule_wireframe_lines_into(
            center,
            half_height,
            radius,
            orientation,
            color,
            &mut self.lines,
        );
    }

    /// The cross is drawn at the raycast hit point to make the precise
    /// impact location obvious.
    pub fn push_cross(&mut self, center: glam::Vec3, half_extent: f32, color: glam::Vec4) {
        cross_lines(center, half_extent, color, &mut self.lines);
    }

    /// The depth attachment is `LoadOp::Load` (preserves the model's depth);
    /// the pipeline's `CompareFunction::Always` depth test draws the wires
    /// on top of the model.
    ///
    /// `camera_uniform` is uploaded to the camera binding
    /// every frame so the debug lines transform with the
    /// same view-proj as the model. Returns immediately
    /// when no lines are buffered.
    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        depth_view: &wgpu::TextureView,
        camera_uniform: &CameraUniform,
    ) {
        if self.lines.is_empty() {
            return;
        }

        // Grow the vertex buffer if the caller pushed
        // more lines than fit. Doubles capacity each time
        // to amortise the allocation cost.
        if self.lines.len() > self.capacity_lines {
            let mut new_capacity = self.capacity_lines.max(1) * 2;
            while new_capacity < self.lines.len() {
                new_capacity *= 2;
            }
            self.vertex_buf = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("debug.vertex_buf"),
                size: (new_capacity * 2 * std::mem::size_of::<DebugVertex>())
                    as wgpu::BufferAddress,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.capacity_lines = new_capacity;
        }

        let mut vertices: Vec<DebugVertex> = Vec::with_capacity(self.lines.len() * 2);
        for line in &self.lines {
            vertices.extend_from_slice(&line.vertices());
        }
        queue.write_buffer(&self.vertex_buf, 0, bytemuck::cast_slice(&vertices));
        queue.write_buffer(&self.camera_buf, 0, bytemuck::bytes_of(camera_uniform));

        let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("debug.pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
                resolve_target: None,
                ops: wgpu::Operations {
                    // Preserve the model's pixels; we're
                    // an overlay, not a clear.
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
                depth_slice: None,
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: depth_view,
                depth_ops: Some(wgpu::Operations {
                    // Read the depth that the VRM pass
                    // wrote; the GPU rejects depth ops
                    // that disagree on Load vs Store, and
                    // the VRM pass uses Store so we
                    // must too.
                    load: wgpu::LoadOp::Load,
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
        rp.set_vertex_buffer(0, self.vertex_buf.slice(..));
        rp.draw(0..(vertices.len() as u32), 0..1);

        drop(rp);

        self.lines.clear();
    }
}

#[cfg(test)]
#[expect(clippy::float_cmp, reason = "test asserts exact float equality")]
mod tests {
    use super::*;
    const SPHERE_LINE_COUNT: usize =
        SPHERE_LONGITUDES * SPHERE_LONGITUDES + (SPHERE_LATITUDES + 1) * SPHERE_LONGITUDES;

    #[test]
    fn sphere_wireframe_lines_lie_on_unit_sphere() {
        let center = glam::Vec3::ZERO;
        let mut lines = Vec::new();
        sphere_wireframe_lines_into(center, 1.0, glam::Vec4::ONE, &mut lines);
        assert_eq!(lines.len(), SPHERE_LINE_COUNT);
        for line in &lines {
            for p in [line.a, line.b] {
                let r2 = p.length_squared();
                assert!(
                    (r2 - 1.0).abs() < 1e-3,
                    "sphere vertex {p:?} has |p|^2 = {r2}, expected ~1.0"
                );
            }
        }
    }

    #[test]
    fn sphere_wireframe_lines_translate_and_scale() {
        let center = glam::Vec3::new(1.0, -2.0, 3.0);
        let radius = 0.5;
        let mut lines = Vec::new();
        sphere_wireframe_lines_into(center, radius, glam::Vec4::ONE, &mut lines);
        for line in &lines {
            for p in [line.a, line.b] {
                let d2 = (p - center).length_squared();
                assert!(
                    (d2 - radius * radius).abs() < 1e-3,
                    "vertex {p:?} is not on the surface of r={radius} at {center:?}, |p-c|^2 = {d2}"
                );
            }
        }
    }

    #[test]
    fn cross_lines_produces_three_segments_one_per_axis() {
        let mut out = Vec::new();
        cross_lines(glam::Vec3::ZERO, 0.05, glam::Vec4::ONE, &mut out);
        assert_eq!(out.len(), 3, "cross must be 3 segments");
        let mut axes_hit = [false; 3];
        for line in &out {
            let delta = line.b - line.a;
            if (delta.x.abs() > delta.y.abs()) && (delta.x.abs() > delta.z.abs()) {
                axes_hit[0] = true;
            } else if (delta.y.abs() > delta.x.abs()) && (delta.y.abs() > delta.z.abs()) {
                axes_hit[1] = true;
            } else {
                axes_hit[2] = true;
            }
        }
        assert!(axes_hit.iter().all(|h| *h), "cross must hit all three axes");
    }

    #[test]
    fn debug_line_vertices_match_endpoints() {
        let line = DebugLine {
            a: glam::Vec3::new(1.0, 2.0, 3.0),
            b: glam::Vec3::new(4.0, 5.0, 6.0),
            color: glam::Vec4::new(0.1, 0.2, 0.3, 0.4),
        };
        let vs = line.vertices();
        assert_eq!(vs[0].pos, [1.0, 2.0, 3.0]);
        assert_eq!(vs[1].pos, [4.0, 5.0, 6.0]);
        assert_eq!(vs[0].color, [0.1, 0.2, 0.3, 0.4]);
        assert_eq!(vs[1].color, [0.1, 0.2, 0.3, 0.4]);
    }

    /// The `bytemuck` derives guarantee `Pod`/`Zeroable` at compile time;
    /// this test only pins the layout size.
    #[test]
    fn debug_vertex_is_28_bytes() {
        assert_eq!(std::mem::size_of::<DebugVertex>(), 28);
    }
}
