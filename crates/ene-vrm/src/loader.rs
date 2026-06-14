//! VRM file loader.
//!
//! Reads a `.vrm` file from disk and produces a [`VrmModel`] with
//! the first mesh's first primitive uploaded to the GPU, the first
//! base-color texture decoded and uploaded, and the first skin's
//! inverse-bind matrices preserved for PR4's skinning pass.
//!
//! PR3 deliberately does **not** support:
//! - Multi-mesh / multi-primitive models (only the first primitive
//!   of the first mesh is rendered).
//! - MToon's full PBR parameters (rim / matcap / outline /
//!   emission). The shader applies a simple diffuse + lit + base
//!   color. The MToon-flavored `KHR_materials_unlit` flag is *read*
//!   so the loader can log a warning if it is set, but does not yet
//!   alter rendering.
//! - Animation, expressions, morph targets, spring bone.
//! - `.gltf` (non-binary) VRM files. Only `.glb` (binary) is
//!   supported in PR3; the glTF binary payload (`BIN` chunk) holds
//!   all the meshes / textures. External `.bin` files require the
//!   `gltf` crate's `import` feature and ship as a follow-up PR.
//!
//! See `docs/architecture/wgpu-migration.md` §22.6 for the PR3 file
//! status.
use std::path::Path;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use image::ImageReader;
use wgpu::util::DeviceExt;

use crate::error::{VrmError, VrmResult};
use crate::model::{MeshVertex, Skeleton, VrmMesh, VrmModel, VrmTexture};

/// Load a `.vrm` file from disk and upload the first mesh +
/// base-color texture to the GPU.
///
/// `path` is the on-disk `.vrm` (a glTF binary with the VRMC_vrm
/// extension). `device` and `queue` are used to allocate the vertex
/// / index / texture buffers.
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

    let mesh = load_first_mesh(&gltf, device)?;
    let base_color = load_first_base_color_texture(&gltf, device, queue)?;
    let skeleton = load_first_skeleton(&gltf);

    for material in gltf.document.materials() {
        if material.unlit() {
            tracing::info!(
                "VRM {} uses KHR_materials_unlit; PR3 shader ignores the flag",
                path.display()
            );
        }
    }

    Ok(VrmModel {
        mesh,
        base_color,
        skeleton,
    })
}

fn load_first_mesh(gltf: &gltf::Gltf, device: &wgpu::Device) -> VrmResult<VrmMesh> {
    let Some(mesh) = gltf.document.meshes().next() else {
        return Err(VrmError::NoMeshes);
    };
    let Some(primitive) = mesh.primitives().next() else {
        return Err(VrmError::NoMeshes);
    };
    if primitive.mode() != gltf::mesh::Mode::Triangles {
        return Err(VrmError::UnsupportedTopology {
            mesh: 0,
            primitive: 0,
        });
    }
    let reader = primitive.reader(|_buffer| gltf.blob.as_deref());

    let positions: Vec<[f32; 3]> = reader
        .read_positions()
        .ok_or(VrmError::NoPositions(0))?
        .collect();
    let normals: Vec<[f32; 3]> = reader
        .read_normals()
        .map(|n| n.collect())
        .unwrap_or_else(|| vec![[0.0, 0.0, 1.0]; positions.len()]);
    let uvs: Vec<[f32; 2]> = reader
        .read_tex_coords(0)
        .map(|tc| tc.into_f32().collect())
        .unwrap_or_else(|| vec![[0.0, 0.0]; positions.len()]);

    let mut vertices = Vec::with_capacity(positions.len());
    for ((pos, normal), uv) in positions.iter().zip(normals.iter()).zip(uvs.iter()) {
        vertices.push(MeshVertex {
            position: *pos,
            uv: *uv,
            normal: *normal,
        });
    }

    let indices: Vec<u32> = reader
        .read_indices()
        .map(|i| i.into_u32().collect())
        .ok_or_else(|| VrmError::Gltf("primitive has no index buffer".into()))?;

    let vertex_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("vrm.vertex_buf"),
        contents: MeshVertex::as_bytes(&vertices),
        usage: wgpu::BufferUsages::VERTEX,
    });
    let index_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("vrm.index_buf"),
        contents: bytemuck::cast_slice(&indices),
        usage: wgpu::BufferUsages::INDEX,
    });

    Ok(VrmMesh {
        vertex_buf,
        vertex_count: vertices.len() as u32,
        index_buf,
        index_count: indices.len() as u32,
    })
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

fn load_first_base_color_texture(
    gltf: &gltf::Gltf,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
) -> VrmResult<Option<VrmTexture>> {
    let Some(material) = gltf.document.materials().next() else {
        return Ok(None);
    };
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
    /// Number of joints in the skeleton. Zero for models with no skin.
    pub fn joint_count(&self) -> usize {
        self.skeleton.inverse_bind.len()
    }
}
