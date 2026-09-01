//! Compare `AliciaSolid` VRM 0.x and 1.0 fixtures when present locally.
//!
//! Expected filenames:
//! - `AliciaSolid0-X.vrm` — legacy VRM 0.x (`extensionsUsed: VRM`)
//! - `AliciaSolid1-0.vrm` — VRM 1.0 (`extensionsUsed: VRMC_vrm`)
//!
//! Reference implementations:
//! - VRM 0.x thumb chain: `LeftThumbProximal` / `Intermediate` / `Distal`
//!   ([bevy_vrm](https://github.com/unavi-xyz/bevy_vrm))
//! - VRM 1.0 + VRMA retargeting: `LeftThumbMetacarpal` / `Proximal` / `Distal`
//!   ([bevy_vrm1](https://github.com/not-elm/bevy_vrm1))
#![cfg_attr(
    test,
    expect(
        clippy::unwrap_used,
        clippy::expect_used,
        reason = "integration tests use unwrap/expect for assertions"
    )
)]

use std::path::{Path, PathBuf};

use ene_vrm::animation::{VrmaAsset, VrmaClip, VrmaFrame, load_vrma};
use ene_vrm::loader::load_vrm;
use ene_vrm::model::VrmFormatVersion;
use glam::Quat;

fn upload(name: &str) -> PathBuf {
    PathBuf::from("/home/ubuntu/.cursor/projects/workspace/uploads").join(name)
}

fn asset(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../assets/characters/Alicia")
        .join(name)
}

fn first_existing(candidates: &[PathBuf]) -> Option<PathBuf> {
    candidates.iter().find(|path| path.is_file()).cloned()
}

fn alicia_vrm0_path() -> Option<PathBuf> {
    first_existing(&[
        upload("AliciaSolid0-X.vrm"),
        upload("AliciaSolid_d551.vrm"),
        asset("AliciaSolid0-X.vrm"),
    ])
}

fn alicia_vrm1_path() -> Option<PathBuf> {
    first_existing(&[
        upload("AliciaSolid1-0.vrm"),
        asset("AliciaSolid1-0.vrm"),
        asset("AliciaSolid.vrm"),
    ])
}

fn vrma_motion_path() -> Option<PathBuf> {
    first_existing(&[asset("motions/VRMA_01.vrma"), upload("VRMA_01.vrma")])
}

fn detect_format_from_glb(path: &Path) -> Option<VrmFormatVersion> {
    let bytes = std::fs::read(path).ok()?;
    if bytes.get(0..4)? != b"glTF" {
        return None;
    }
    let chunk0_len = u32::from_le_bytes(bytes.get(12..16)?.try_into().ok()?);
    let json = bytes.get(20..20 + chunk0_len as usize)?;
    let gltf: serde_json::Value = serde_json::from_slice(json).ok()?;
    let used = gltf
        .get("extensionsUsed")?
        .as_array()?
        .iter()
        .filter_map(|v| v.as_str())
        .collect::<Vec<_>>();
    if used.contains(&"VRMC_vrm") {
        Some(VrmFormatVersion::V1)
    } else if used.contains(&"VRM") {
        Some(VrmFormatVersion::V0x)
    } else {
        None
    }
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
                label: Some("ene-vrm-alicia-format-compare"),
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

#[test]
fn alicia_vrm0_fixture_declares_legacy_extension() {
    let Some(path) = alicia_vrm0_path() else {
        return;
    };
    assert_eq!(
        detect_format_from_glb(&path),
        Some(VrmFormatVersion::V0x),
        "{} should be VRM 0.x (root extension VRM)",
        path.display()
    );
}

#[test]
fn alicia_vrm1_fixture_declares_vrmc_extension() {
    let Some(path) = alicia_vrm1_path() else {
        return;
    };
    assert_eq!(
        detect_format_from_glb(&path),
        Some(VrmFormatVersion::V1),
        "{} should be VRM 1.0 (root extension VRMC_vrm)",
        path.display()
    );
}

#[test]
fn alicia_thumb_topology_matches_reference_repos() {
    let Some((device, queue)) = try_create_wgpu_device() else {
        return;
    };

    if let Some(path) = alicia_vrm0_path() {
        let model = load_vrm(&path, &device, &queue).unwrap();
        assert_eq!(model.format_version(), VrmFormatVersion::V0x);
        assert!(
            model.humanoid.by_name("leftThumbIntermediate").is_some(),
            "VRM 0.x should expose leftThumbIntermediate (bevy_vrm chain)"
        );
        assert!(
            model.humanoid.by_name("leftThumbMetacarpal").is_none(),
            "VRM 0.x should not expose leftThumbMetacarpal"
        );
    }

    if let Some(path) = alicia_vrm1_path() {
        let model = load_vrm(&path, &device, &queue).unwrap();
        assert_eq!(model.format_version(), VrmFormatVersion::V1);
        assert!(
            model.humanoid.by_name("leftThumbMetacarpal").is_some(),
            "VRM 1.0 should expose leftThumbMetacarpal (bevy_vrm1 chain)"
        );
        assert!(
            model.humanoid.by_name("leftThumbIntermediate").is_none(),
            "VRM 1.0 should not expose leftThumbIntermediate"
        );
    }
}

#[test]
fn vrma_left_thumb_proximal_targets_different_nodes_by_format() {
    let Some((device, queue)) = try_create_wgpu_device() else {
        return;
    };
    let Some(vrm0_path) = alicia_vrm0_path() else {
        return;
    };
    let Some(vrm1_path) = alicia_vrm1_path() else {
        return;
    };
    let Some(vrma_path) = vrma_motion_path() else {
        return;
    };

    let vrm0 = load_vrm(&vrm0_path, &device, &queue).unwrap();
    let vrm1 = load_vrm(&vrm1_path, &device, &queue).unwrap();
    let asset: VrmaAsset = load_vrma(&vrma_path).unwrap();
    let clip: &VrmaClip = asset.clips.first().expect("VRMA clip");

    let frame0: VrmaFrame = vrm0.evaluate_vrma(&asset, clip, 0.5);
    let frame1: VrmaFrame = vrm1.evaluate_vrma(&asset, clip, 0.5);

    let vrm0_intermediate = vrm0
        .humanoid
        .by_name("leftThumbIntermediate")
        .expect("vrm0 intermediate")
        .node;
    let vrm1_proximal = vrm1
        .humanoid
        .by_name("leftThumbProximal")
        .expect("vrm1 proximal")
        .node;

    assert!(
        frame0.bone_rotations.contains_key("leftthumbintermediate"),
        "VRMA leftthumbproximal should retarget to VRM0 intermediate, got {:?}",
        frame0.bone_rotations.keys().collect::<Vec<_>>()
    );
    assert!(
        frame1.bone_rotations.contains_key("leftthumbproximal"),
        "VRMA leftthumbproximal should stay on VRM1 proximal, got {:?}",
        frame1.bone_rotations.keys().collect::<Vec<_>>()
    );
    assert_ne!(
        vrm0_intermediate, vrm1_proximal,
        "comparison requires distinct thumb nodes between formats"
    );

    if let Some(rot) = frame0.bone_rotations.get("leftthumbintermediate") {
        assert_ne!(*rot, Quat::IDENTITY, "VRM0 thumb track should be non-identity at t=0.5");
    }
}
