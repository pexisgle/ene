//! Skeleton debug overlay glue.
//!
//! Walks the loaded [`VrmModel`]'s humanoid bone hierarchy and its
//! spring-bone chains, pushing a red [`DebugLine`] per bone segment
//! so the rig can be inspected in the character-window debug overlay.
//!
//! Bones that have a humanoid child draw a single parent→child
//! segment; leaf bones draw a small 3-axis cross instead so they
//! remain visible. Spring-bone chains draw one segment per joint pair.
use ene_vrm::VrmModel;
use ene_vrm::debug_renderer::DebugLine;

const BONE_COLOR: glam::Vec4 = glam::Vec4::new(1.0, 0.0, 0.0, 1.0);

/// Half-extent of the leaf-bone cross, as a fraction of the
/// character's actual scale.
const LEAF_CROSS_EXTENT: f32 = 0.01;

/// `character_position` and `actual_scale` place the rig in world
/// space; the model's own centre and normalize scale are read from
/// `model` so raw vertex positions land at their rendered location.
pub fn build_skeleton_lines(
    lines: &mut Vec<DebugLine>,
    model: &VrmModel,
    character_position: glam::Vec3,
    actual_scale: f32,
) {
    let center = glam::Vec3::from(model.center());
    let normalize_scale = model.normalize_scale();

    let to_world =
        |raw: glam::Vec3| character_position + (raw - center) * normalize_scale * actual_scale;

    for (bone_name, entry) in model.humanoid.iter() {
        let parent_world = to_world(model.nodes.world_positions[entry.node]);

        if let Some(child_node) =
            crate::character::collider::get_humanoid_child_node(bone_name.as_str(), &model.humanoid)
        {
            let child_world = to_world(model.nodes.world_positions[child_node]);
            lines.push(DebugLine {
                a: parent_world,
                b: child_world,
                color: BONE_COLOR,
            });
        } else {
            push_leaf_cross(lines, parent_world, LEAF_CROSS_EXTENT * actual_scale);
        }
    }

    if let Some(spring_bones) = &model.spring_bones {
        for chain in &spring_bones.springs {
            for pair in chain.joints.windows(2) {
                let parent_world = to_world(model.nodes.world_positions[pair[0].node]);
                let child_world = to_world(model.nodes.world_positions[pair[1].node]);
                lines.push(DebugLine {
                    a: parent_world,
                    b: child_world,
                    color: BONE_COLOR,
                });
            }
        }
    }
}

/// Pushes a small 3-axis cross centred on `point` so leaf bones
/// (which have no child segment) stay visible in the overlay.
fn push_leaf_cross(lines: &mut Vec<DebugLine>, point: glam::Vec3, extent: f32) {
    for axis in [glam::Vec3::X, glam::Vec3::Y, glam::Vec3::Z] {
        lines.push(DebugLine {
            a: point - axis * extent,
            b: point + axis * extent,
            color: BONE_COLOR,
        });
    }
}
