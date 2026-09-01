//! Load-test the user-provided `AliciaSolid` VRM 0.x fixture.
#![cfg_attr(
    test,
    expect(
        clippy::unwrap_used,
        clippy::expect_used,
        reason = "integration tests use unwrap/expect for assertions"
    )
)]

use std::path::PathBuf;

use ene_vrm::loader::load_vrm;
use ene_vrm::look_at::{LookAtEvaluator, LookAtType};
use ene_vrm::model::VrmFormatVersion;
use ene_vrm::spring_bone::SpringBoneSimulator;
use glam::{Quat, Vec3};

fn alicia_path() -> PathBuf {
    PathBuf::from("/home/ubuntu/.cursor/projects/workspace/uploads/AliciaSolid_d551.vrm")
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
                label: Some("ene-vrm-alicia-vrm0-test"),
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
fn alicia_solid_vrm0_loads_and_maps_runtime_expressions() {
    let Some((device, queue)) = try_create_wgpu_device() else {
        return;
    };
    let path = alicia_path();
    if !path.exists() {
        return;
    }

    let model = load_vrm(&path, &device, &queue).unwrap();
    assert_eq!(model.format_version(), VrmFormatVersion::V0x);
    assert!(
        model.humanoid.len() >= 50,
        "humanoid bones = {}",
        model.humanoid.len()
    );
    for bone in ["hips", "head", "lefteye", "righteye"] {
        assert!(
            model.humanoid.by_name(bone).is_some(),
            "missing humanoid bone {bone}"
        );
    }
    for bone in [
        "leftThumbProximal",
        "leftThumbIntermediate",
        "leftThumbDistal",
    ] {
        assert!(
            model.humanoid.by_name(bone).is_some(),
            "missing thumb bone {bone}"
        );
    }
    assert_eq!(
        model.humanoid.by_name("leftThumbProximal").unwrap().node,
        70
    );
    assert_eq!(
        model
            .humanoid
            .by_name("leftThumbIntermediate")
            .unwrap()
            .node,
        71
    );

    let names: Vec<&str> = model
        .expressions_meta
        .iter()
        .map(|e| e.name.as_str())
        .collect();
    for expected in [
        "happy",
        "sad",
        "blink",
        "lookUp",
        "lookDown",
        "lookLeft",
        "lookRight",
    ] {
        assert!(
            names.contains(&expected),
            "expression {expected} missing from converted presets {names:?}"
        );
    }

    let look_at = model.look_at.as_ref().expect("lookAt");
    assert_eq!(look_at.look_at_type, LookAtType::Bone);
    assert!((look_at.offset_from_head_bone[1] - 0.06).abs() < 0.01);

    assert!(
        model
            .spring_bones
            .as_ref()
            .is_some_and(|sb| !sb.springs.is_empty())
    );
    assert!(model.joint_count() > 0);
}

#[test]
fn alicia_solid_vrm0_runtime_pose_and_springs_step() {
    let Some((device, queue)) = try_create_wgpu_device() else {
        return;
    };
    let path = alicia_path();
    if !path.exists() {
        return;
    }

    let mut model = load_vrm(&path, &device, &queue).unwrap();
    let props = model.look_at.unwrap_or_default();
    let evaluator = LookAtEvaluator::new(&props);
    let head = model
        .humanoid
        .head()
        .and_then(|entry| model.nodes.world_positions.get(entry.node).copied())
        .unwrap_or(Vec3::new(0.0, 1.2, 0.0));
    let look_at = evaluator.evaluate(head, head + Vec3::new(1.0, 0.0, 1.0), Quat::IDENTITY);
    let bone = match look_at {
        ene_vrm::look_at::LookAtOutput::Bone(b) => Some(b),
        _ => None,
    };
    let palette = model.update_skin_palette(
        &ene_vrm::animation::VrmaFrame {
            bone_rotations: std::collections::HashMap::default(),
            hips_translation: None,
            expression_weights: std::collections::HashMap::default(),
            look_at_yaw_pitch: None,
        },
        bone.as_ref(),
    );
    assert!(!palette.is_empty());

    let spring_props = model.spring_bones.clone().expect("spring bones");
    let pos: std::collections::HashMap<_, _> = model
        .nodes
        .world_positions
        .iter()
        .enumerate()
        .map(|(i, p)| (i, *p))
        .collect();
    let rot: std::collections::HashMap<_, _> = model
        .nodes
        .world_rotations
        .iter()
        .enumerate()
        .map(|(i, r)| (i, *r))
        .collect();
    let parent_rot: std::collections::HashMap<_, _> = model
        .nodes
        .parents
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let parent = if *p >= 0 {
                rot.get(&(*p as usize)).copied().unwrap_or(Quat::IDENTITY)
            } else {
                Quat::IDENTITY
            };
            (i, parent)
        })
        .collect();
    let local_rot: std::collections::HashMap<_, _> = model
        .nodes
        .rest_local_rotations
        .iter()
        .enumerate()
        .map(|(i, r)| (i, *r))
        .collect();
    let mut sim = SpringBoneSimulator::new(&spring_props, &pos, &rot, &parent_rot, &local_rot);
    let updates = sim.step(
        1.0 / 60.0,
        &spring_props,
        &pos,
        &rot,
        &parent_rot,
        &pos,
        &rot,
    );
    assert!(
        !updates.is_empty(),
        "spring bone sim should move hair joints"
    );
}
