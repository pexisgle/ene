//! VRM file loader.
//!
//! Reads a `.vrm` file from disk and produces a [`VrmModel`] with
//! every primitive of **every** glTF `Mesh` uploaded to the GPU
//! (vertex / index buffers + each primitive's own base-color
//! texture) and the first skin's inverse-bind matrices preserved
//! for PR4's skinning pass.
//!
//! ## Multi-mesh scope (PR3.3)
//!
//! A VRM 1.0 file such as `AliciaSolid.vrm` contains ~12 separate
//! glTF `Mesh` objects (body, hair, face, clothes, accessories,
//! …), not one mesh with many primitives. PR3.0/3.1/3.2 only
//! loaded `meshes[0]` (the head/face area) and therefore rendered
//! only the skin. PR3.3 walks the entire `gltf.meshes()` iterator,
//! computing one global AABB across all primitive positions so the
//! per-vertex normalization (`TARGET_MODEL_SIZE = 1.5` m) is
//! applied uniformly across the whole body.
//!
//! ## PR3.x deliberately does **not** support
//!
//! - MToon's full PBR parameters (rim / matcap / outline /
//!   emission). The shader applies a simple diffuse + lit + base
//!   color. The MToon-flavored `KHR_materials_unlit` flag is
//!   *read* so the loader can log a warning if it is set, but
//!   does not yet alter rendering.
//! - Per-mesh glTF node transforms. The PR3.3 loader uses raw
//!   vertex positions from each mesh without applying the glTF
//!   node hierarchy's world transforms. For VRoid-exported models
//!   this is fine because the body parts are already in
//!   world-space positions in their local vertex buffers. The
//!   humanoid bone system (PR4) will eventually apply the
//!   per-joint skinning matrices on top of this.
//! - Animation, expressions, morph targets, spring bone.
//! - `.gltf` (non-binary) VRM files. Only `.glb` (binary) is
//!   supported in PR3.x; the glTF binary payload (`BIN` chunk)
//!   holds all the meshes / textures. External `.bin` files
//!   require the `gltf` crate's `import` feature and ship as a
//!   follow-up PR.
//!
//! See `docs/architecture/wgpu-migration.md` §22.6 for the PR3
//! status.
use std::path::Path;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use image::ImageReader;
use wgpu::util::DeviceExt;

use crate::error::{VrmError, VrmResult};
use crate::model::{MeshVertex, Skeleton, VrmMesh, VrmModel, VrmPrimitive, VrmTexture};

/// Target world-space size of the model along its longest axis
/// after PR3.1's per-vertex normalization. The legacy Bevy
/// `bevy_vrm1` bakes the same scale into its world transform;
/// v2 applies it at load time.
const TARGET_MODEL_SIZE: f32 = 1.5;

/// Load a `.vrm` file from disk and upload every primitive of the
/// first mesh to the GPU.
///
/// `path` is the on-disk `.vrm` (a glTF binary with the VRMC_vrm
/// extension). `device` and `queue` are used to allocate the
/// vertex / index / texture buffers.
pub fn load_vrm(
    path: impl AsRef<Path>,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
) -> VrmResult<VrmModel> {
    let path = path.as_ref();
    let bytes = std::fs::read(path).map_err(|source| VrmError::Io {
        path: path.display().to_string(),
        source,
    })?;

    let gltf = gltf::Gltf::from_slice(&bytes).map_err(|e| VrmError::Gltf(e.to_string()))?;

    if !gltf.document.extensions_used().any(|e| e == "VRMC_vrm") {
        return Err(VrmError::NotVrm);
    }

    let (mesh, aabb_min, aabb_max) = load_all_meshes(&gltf, device, queue)?;
    let skeleton = load_first_skeleton(&gltf);

    for material in gltf.document.materials() {
        if material.unlit() {
            tracing::info!(
                "VRM {} uses KHR_materials_unlit; PR3 shader ignores the flag",
                path.display()
            );
        }
    }

    Ok(VrmModel::new(mesh, skeleton, aabb_min, aabb_max))
}

/// Load every triangle-list primitive of every glTF `Mesh` in the
/// document. Returns a `Vec<VrmMesh>` — one entry per glTF `Mesh`.
/// Each primitive gets its own vertex / index buffers and (when
/// its material has one) its own base-color texture, so the body,
/// clothes, face, hair, and accessories all render.
fn load_all_meshes(
    gltf: &gltf::Gltf,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
) -> VrmResult<(Vec<VrmMesh>, [f32; 3], [f32; 3])> {
    // First pass: collect every triangle-list primitive's
    // positions across **all** glTF meshes so we can compute one
    // global AABB and apply a uniform normalize-scale. (A VRM
    // 1.0 has ~12 separate glTF Mesh objects, not one.)
    let mut all_positions: Vec<[f32; 3]> = Vec::new();
    for mesh in gltf.document.meshes() {
        for primitive in mesh.primitives() {
            if primitive.mode() != gltf::mesh::Mode::Triangles {
                continue;
            }
            let reader = primitive.reader(|_buffer| gltf.blob.as_deref());
            if let Some(positions) = reader.read_positions() {
                all_positions.extend(positions);
            }
        }
    }
    if all_positions.is_empty() {
        return Err(VrmError::NoMeshes);
    }

    let mut bb_min = all_positions[0];
    let mut bb_max = all_positions[0];
    for p in &all_positions {
        for i in 0..3 {
            bb_min[i] = bb_min[i].min(p[i]);
            bb_max[i] = bb_max[i].max(p[i]);
        }
    }
    let extent: [f32; 3] = [
        bb_max[0] - bb_min[0],
        bb_max[1] - bb_min[1],
        bb_max[2] - bb_min[2],
    ];
    let max_extent = extent.iter().copied().fold(0.0f32, f32::max);
    let center: [f32; 3] = [
        (bb_min[0] + bb_max[0]) * 0.5,
        (bb_min[1] + bb_max[1]) * 0.5,
        (bb_min[2] + bb_max[2]) * 0.5,
    ];
    let scale = if max_extent > 0.0001 {
        TARGET_MODEL_SIZE / max_extent
    } else {
        1.0
    };
    let total_mesh_count = gltf.document.meshes().count();
    let total_primitive_count: usize = gltf.document.meshes().map(|m| m.primitives().count()).sum();
    tracing::info!(
        "VRM AABB: min={:?} max={:?} extent={:?} max_extent={} center={:?} normalize_scale={} mesh_count={} primitive_count={}",
        bb_min,
        bb_max,
        extent,
        max_extent,
        center,
        scale,
        total_mesh_count,
        total_primitive_count
    );

    // Second pass: build one `VrmMesh` per glTF mesh, each holding
    // its triangle-list primitives. The same `(center, scale)`
    // transform is applied to every vertex so the whole body
    // ends up centered on origin and bounded by the target size.
    let mut meshes = Vec::new();
    for (mesh_idx, mesh) in gltf.document.meshes().enumerate() {
        let mut primitives = Vec::new();
        for (prim_idx, primitive) in mesh.primitives().enumerate() {
            if primitive.mode() != gltf::mesh::Mode::Triangles {
                tracing::warn!(
                    "VRM mesh[{mesh_idx}].primitive[{prim_idx}] uses unsupported topology {:?}; skipping",
                    primitive.mode()
                );
                continue;
            }
            let reader = primitive.reader(|_buffer| gltf.blob.as_deref());

            let positions: Vec<[f32; 3]> = match reader.read_positions() {
                Some(p) => p.collect(),
                None => {
                    tracing::warn!(
                        "VRM mesh[{mesh_idx}].primitive[{prim_idx}] has no POSITION; skipping"
                    );
                    continue;
                }
            };
            let normals: Vec<[f32; 3]> = reader
                .read_normals()
                .map(|n| n.collect())
                .unwrap_or_else(|| vec![[0.0, 0.0, 1.0]; positions.len()]);
            let uvs: Vec<[f32; 2]> = reader
                .read_tex_coords(0)
                .map(|tc| tc.into_f32().collect())
                .unwrap_or_else(|| vec![[0.0, 0.0]; positions.len()]);
            let indices: Vec<u32> = match reader.read_indices() {
                Some(i) => i.into_u32().collect(),
                None => {
                    tracing::warn!(
                        "VRM mesh[{mesh_idx}].primitive[{prim_idx}] has no index buffer; skipping"
                    );
                    continue;
                }
            };
            if indices.is_empty() {
                continue;
            }

            let mut vertices = Vec::with_capacity(positions.len());
            for ((pos, normal), uv) in positions.iter().zip(normals.iter()).zip(uvs.iter()) {
                vertices.push(MeshVertex {
                    position: [
                        (pos[0] - center[0]) * scale,
                        (pos[1] - center[1]) * scale,
                        (pos[2] - center[2]) * scale,
                    ],
                    uv: *uv,
                    normal: *normal,
                });
            }

            let vertex_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(&format!("vrm.mesh{mesh_idx}.prim{prim_idx}.vertex_buf")),
                contents: MeshVertex::as_bytes(&vertices),
                usage: wgpu::BufferUsages::VERTEX,
            });
            let index_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(&format!("vrm.mesh{mesh_idx}.prim{prim_idx}.index_buf")),
                contents: bytemuck::cast_slice(&indices),
                usage: wgpu::BufferUsages::INDEX,
            });

            let base_color = match load_primitive_base_color_texture(
                &primitive, gltf, device, queue,
            ) {
                Ok(t) => t,
                Err(e) => {
                    tracing::warn!(
                        "VRM mesh[{mesh_idx}].primitive[{prim_idx}] base-color decode failed: {e}"
                    );
                    None
                }
            };

            primitives.push(VrmPrimitive {
                vertex_buf,
                vertex_count: vertices.len() as u32,
                index_buf,
                index_count: indices.len() as u32,
                base_color,
            });
        }
        if primitives.is_empty() {
            tracing::debug!("VRM mesh[{mesh_idx}] has no renderable primitives; skipping");
            continue;
        }
        meshes.push(VrmMesh { primitives });
    }

    if meshes.is_empty() {
        return Err(VrmError::NoMeshes);
    }

    // Compute the AABB of the **normalized** vertices so the
    // runtime can log it as a diagnostic. Without this the loader
    // only ever logs the raw AABB (before centering + scaling).
    // Derived analytically from the raw AABB + center + scale,
    // which is exact because every vertex was transformed by
    // `(pos - center) * scale` (linear transform preserves AABB
    // shape up to axis sign).
    let post_min = [
        (bb_min[0] - center[0]) * scale,
        (bb_min[1] - center[1]) * scale,
        (bb_min[2] - center[2]) * scale,
    ];
    let post_max = [
        (bb_max[0] - center[0]) * scale,
        (bb_max[1] - center[1]) * scale,
        (bb_max[2] - center[2]) * scale,
    ];
    Ok((meshes, post_min, post_max))
}

fn load_first_skeleton(gltf: &gltf::Gltf) -> Skeleton {
    let mut skel = Skeleton::default();
    let Some(skin) = gltf.document.skins().next() else {
        return skel;
    };
    let reader = skin.reader(|_buffer| gltf.blob.as_deref());
    if let Some(ibm) = reader.read_inverse_bind_matrices() {
        skel.inverse_bind = ibm.map(|m| glam::Mat4::from_cols_array_2d(&m)).collect();
    }
    skel
}

fn load_primitive_base_color_texture(
    primitive: &gltf::Primitive,
    gltf: &gltf::Gltf,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
) -> VrmResult<Option<VrmTexture>> {
    let material = primitive.material();
    let pbr = material.pbr_metallic_roughness();
    let Some(info) = pbr.base_color_texture() else {
        return Ok(None);
    };
    let texture = info.texture();
    let image =
        load_image_data(&texture, gltf).map_err(|e| VrmError::TextureDecode(e.to_string()))?;

    let (width, height) = (image.width, image.height);
    let gpu_texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("vrm.base_color"),
        size: wgpu::Extent3d {
            width,
            height,
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
            texture: &gpu_texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &image.rgba,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(4 * width),
            rows_per_image: Some(height),
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );

    let view = gpu_texture.create_view(&wgpu::TextureViewDescriptor::default());
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("vrm.base_color.sampler"),
        address_mode_u: wgpu::AddressMode::ClampToEdge,
        address_mode_v: wgpu::AddressMode::ClampToEdge,
        address_mode_w: wgpu::AddressMode::ClampToEdge,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        mipmap_filter: wgpu::MipmapFilterMode::Nearest,
        ..Default::default()
    });

    let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("vrm.base_color.bgl"),
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

    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("vrm.base_color.bg"),
        layout: &bind_group_layout,
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

    Ok(Some(VrmTexture {
        texture: gpu_texture,
        sampler,
        bind_group_layout,
        bind_group,
    }))
}

struct DecodedImage {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
}

fn load_image_data(
    texture: &gltf::texture::Texture,
    gltf: &gltf::Gltf,
) -> Result<DecodedImage, String> {
    let source = texture.source();
    let data = match source.source() {
        gltf::image::Source::Uri { uri, mime_type: _ } => {
            if let Some(rest) = uri.strip_prefix("data:") {
                let (_meta, b64) = rest
                    .split_once(',')
                    .ok_or_else(|| "data URI missing ','".to_string())?;
                BASE64_STANDARD
                    .decode(b64)
                    .map_err(|e| format!("base64 decode: {e}"))?
            } else {
                return Err(format!("external image URI not supported: {uri}"));
            }
        }
        gltf::image::Source::View { view, mime_type: _ } => {
            let parent_buffer = view.buffer();
            if parent_buffer.index() != 0 {
                return Err(format!(
                    "only buffer index 0 is supported (got {})",
                    parent_buffer.index()
                ));
            }
            let blob = gltf
                .blob
                .as_ref()
                .ok_or_else(|| "buffer referenced but no GLB blob present".to_string())?;
            let start = view.offset();
            let end = start + view.length();
            blob.get(start..end)
                .ok_or_else(|| format!("buffer view out of range: {}..{}", start, end))?
                .to_vec()
        }
    };

    let cursor = std::io::Cursor::new(data);
    let reader = ImageReader::new(cursor)
        .with_guessed_format()
        .map_err(|e| e.to_string())?;
    let img = reader.decode().map_err(|e| format!("PNG decode: {e}"))?;
    let rgba = img.to_rgba8();
    Ok(DecodedImage {
        width: rgba.width(),
        height: rgba.height(),
        rgba: rgba.into_raw(),
    })
}

impl VrmModel {
    // `joint_count` is defined in `model.rs` alongside the rest of
    // the public API.
}
