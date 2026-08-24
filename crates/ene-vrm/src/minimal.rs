//! Minimal VRM 1.0 GLB fixture for tests and stage bootstrap.
//!
//! Produces a tiny binary glTF with the `VRMC_vrm` extension and a
//! single quad mesh — enough for [`crate::loader::load_vrm`] to succeed
//! without shipping a large bundled character model.

use std::path::Path;

use crate::error::{VrmError, VrmResult};

/// Returns a minimal valid VRM 1.0 GLB (binary glTF with `VRMC_vrm`).
///
/// The asset contains one quad (two triangles) with `POSITION` /
/// `NORMAL` / `TEXCOORD_0` attributes and a default PBR material.
/// No textures, skin, or morph targets.
#[must_use]
pub fn minimal_vrm_glb_bytes() -> Vec<u8> {
    pack_glb(GLTF_JSON.as_bytes(), &mesh_bin_chunk())
}

/// Write [`minimal_vrm_glb_bytes`] to `path`.
pub fn write_glb(path: impl AsRef<Path>) -> VrmResult<()> {
    let path = path.as_ref();
    std::fs::write(path, minimal_vrm_glb_bytes()).map_err(|source| VrmError::Io {
        path: path.display().to_string(),
        source,
    })
}

const GLTF_JSON: &str = r#"{"asset":{"version":"2.0","generator":"ene-vrm minimal fixture"},"extensionsUsed":["VRMC_vrm"],"extensions":{"VRMC_vrm":{"specVersion":"1.0","meta":{"name":"minimal"},"humanoid":{"humanBones":{}}}},"scene":0,"scenes":[{"nodes":[0]}],"nodes":[{"mesh":0,"name":"mesh"}],"meshes":[{"name":"quad","primitives":[{"attributes":{"POSITION":0,"NORMAL":1,"TEXCOORD_0":2},"indices":3,"material":0}]}],"materials":[{"name":"default","pbrMetallicRoughness":{"baseColorFactor":[0.8,0.6,0.9,1.0],"metallicFactor":0.0,"roughnessFactor":0.9}}],"buffers":[{"byteLength":140}],"bufferViews":[{"buffer":0,"byteOffset":0,"byteLength":48,"target":34962},{"buffer":0,"byteOffset":48,"byteLength":48,"target":34962},{"buffer":0,"byteOffset":96,"byteLength":32,"target":34962},{"buffer":0,"byteOffset":128,"byteLength":12,"target":34963}],"accessors":[{"bufferView":0,"componentType":5126,"count":4,"type":"VEC3","min":[-0.5,0.0,0.0],"max":[0.5,1.0,0.0]},{"bufferView":1,"componentType":5126,"count":4,"type":"VEC3"},{"bufferView":2,"componentType":5126,"count":4,"type":"VEC2"},{"bufferView":3,"componentType":5123,"count":6,"type":"SCALAR"}]}"#;

fn mesh_bin_chunk() -> [u8; 140] {
    let positions: [[f32; 3]; 4] = [
        [-0.5, 0.0, 0.0],
        [0.5, 0.0, 0.0],
        [0.5, 1.0, 0.0],
        [-0.5, 1.0, 0.0],
    ];
    let normals = [[0.0_f32, 0.0, 1.0]; 4];
    let uvs: [[f32; 2]; 4] = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
    let indices: [u16; 6] = [0, 1, 2, 0, 2, 3];

    let mut bin = [0_u8; 140];
    let mut offset = 0usize;

    for vertex in &positions {
        for component in vertex {
            bin[offset..offset + 4].copy_from_slice(&component.to_le_bytes());
            offset += 4;
        }
    }
    for vertex in &normals {
        for component in vertex {
            bin[offset..offset + 4].copy_from_slice(&component.to_le_bytes());
            offset += 4;
        }
    }
    for vertex in &uvs {
        for component in vertex {
            bin[offset..offset + 4].copy_from_slice(&component.to_le_bytes());
            offset += 4;
        }
    }
    for index in &indices {
        bin[offset..offset + 2].copy_from_slice(&index.to_le_bytes());
        offset += 2;
    }

    debug_assert_eq!(offset, 140);
    bin
}

fn pack_glb(json: &[u8], bin: &[u8]) -> Vec<u8> {
    let json_pad = (4 - (json.len() % 4)) % 4;
    let bin_pad = (4 - (bin.len() % 4)) % 4;
    let total_len = 12 + 8 + json.len() + json_pad + 8 + bin.len() + bin_pad;

    let mut out = Vec::with_capacity(total_len);
    out.extend_from_slice(b"glTF");
    out.extend_from_slice(&2u32.to_le_bytes());
    out.extend_from_slice(&(total_len as u32).to_le_bytes());

    out.extend_from_slice(&(json.len() as u32).to_le_bytes());
    out.extend_from_slice(b"JSON");
    out.extend_from_slice(json);
    out.extend(std::iter::repeat_n(0_u8, json_pad));

    out.extend_from_slice(&(bin.len() as u32).to_le_bytes());
    out.extend_from_slice(b"BIN\0");
    out.extend_from_slice(bin);
    out.extend(std::iter::repeat_n(0_u8, bin_pad));

    debug_assert_eq!(out.len(), total_len);
    out
}

#[cfg(test)]
mod tests {
    use crate::loader::load_vrm;

    use super::{minimal_vrm_glb_bytes, write_glb};

    #[test]
    fn minimal_glb_parses_as_vrm() {
        let bytes = minimal_vrm_glb_bytes();
        assert!(bytes.starts_with(b"glTF"));

        let gltf = gltf::Gltf::from_slice(&bytes).expect("glb parse");
        assert!(
            gltf.document
                .extensions_used()
                .any(|extension| extension == "VRMC_vrm")
        );
        assert_eq!(gltf.document.meshes().count(), 1);
        assert!(gltf.blob.is_some(), "binary glb must carry a BIN chunk");
    }

    #[test]
    fn write_glb_round_trip() {
        let path = std::env::temp_dir().join(format!("ene_vrm_minimal_{}.vrm", std::process::id()));
        write_glb(&path).expect("write");
        let bytes = std::fs::read(&path).expect("read");
        let gltf = gltf::Gltf::from_slice(&bytes).expect("parse");
        assert!(
            gltf.document
                .extensions_used()
                .any(|extension| extension == "VRMC_vrm")
        );
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn minimal_glb_loads_with_wgpu() {
        let Some((device, queue)) = try_create_wgpu_device() else {
            return;
        };

        let path =
            std::env::temp_dir().join(format!("ene_vrm_minimal_load_{}.vrm", std::process::id()));
        write_glb(&path).expect("write");
        let model = load_vrm(&path, &device, &queue).expect("load_vrm");
        assert!(!model.meshes.is_empty());
        assert_eq!(model.meshes[0].primitives.len(), 1);
        let _renderer = crate::renderer::VrmRenderer::new(
            &device,
            &queue,
            wgpu::TextureFormat::Rgba8UnormSrgb,
            None,
            &model,
        );
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn shipped_alicia_vrm_parses_and_loads() {
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../assets/characters/Alicia/AliciaSolid.vrm");
        if !path.is_file() {
            // The model is license-restricted and not distributed with the
            // repository; fetch it locally via scripts/fetch-character-assets.sh.
            eprintln!("skipping: AliciaSolid.vrm not present (see scripts/fetch-character-assets.sh)");
            return;
        }
        let bytes = std::fs::read(&path).expect("read AliciaSolid.vrm");
        let gltf = gltf::Gltf::from_slice(&bytes).expect("AliciaSolid.vrm should be valid glTF");
        assert!(
            gltf.document
                .extensions_used()
                .any(|extension| extension == "VRMC_vrm"),
            "shipped Alicia VRM should declare VRMC_vrm"
        );
        assert!(
            gltf.document.meshes().count() > 1,
            "Alicia is a multi-mesh VRM, not the single-quad fixture"
        );

        let Some((device, queue)) = try_create_wgpu_device() else {
            return;
        };
        let model = load_vrm(&path, &device, &queue).expect("AliciaSolid.vrm should load on wgpu");
        assert!(
            model.meshes.len() > 1,
            "loader must keep every Alicia mesh, not only meshes[0]"
        );
        let _renderer = crate::renderer::VrmRenderer::new(
            &device,
            &queue,
            wgpu::TextureFormat::Rgba8UnormSrgb,
            None,
            &model,
        );
    }

    fn try_create_wgpu_device() -> Option<(wgpu::Device, wgpu::Queue)> {
        pollster::block_on(async {
            let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
                backends: wgpu::Backends::PRIMARY,
                backend_options: wgpu::BackendOptions::default(),
                flags: wgpu::InstanceFlags::default(),
                display: None,
                memory_budget_thresholds: wgpu::MemoryBudgetThresholds::default(),
            });
            let adapter = instance
                .request_adapter(&wgpu::RequestAdapterOptions {
                    power_preference: wgpu::PowerPreference::LowPower,
                    compatible_surface: None,
                    force_fallback_adapter: true,
                })
                .await
                .ok()?;
            adapter
                .request_device(&wgpu::DeviceDescriptor {
                    label: Some("ene-vrm-minimal-test"),
                    required_features: wgpu::Features::empty(),
                    required_limits: {
                        let mut limits =
                            wgpu::Limits::downlevel_defaults().using_resolution(adapter.limits());
                        limits.max_bind_groups = adapter.limits().max_bind_groups;
                        limits
                    },
                    memory_hints: wgpu::MemoryHints::default(),
                    experimental_features: wgpu::ExperimentalFeatures::disabled(),
                    trace: wgpu::Trace::Off,
                })
                .await
                .ok()
        })
    }
}
