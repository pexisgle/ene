//! Physics-related components.
//!
//! Each component mirrors a piece of the Rapier state for a single
//! character entity. The [`crate::plugin::PhysicsPlugin`]
//! populates these from the [`crate::component::character::BoneColliders`]
//! specs and the per-frame systems consume them to drive the
//! Rapier body and colliders.

use bevy_ecs::prelude::Component;
use glam::{Quat, Vec3};
use rapier3d::prelude::{ColliderHandle, RigidBodyHandle};

/// The Rapier rigid body handle for a character entity's bone
/// collider set. Populated by the physics plugin after the
/// per-bone specs are turned into Rapier colliders.
#[derive(Component)]
pub struct PhysicsBody(
    #[expect(dead_code, reason = "yet to be wired to step/update systems")] pub RigidBodyHandle,
);

/// One Rapier `ColliderHandle` per bone of the character.
#[derive(Component, Debug, Default, Clone)]
pub struct PhysicsColliders(
    #[expect(dead_code, reason = "yet to be wired to step/update systems")] pub Vec<ColliderHandle>,
);

/// Per-bone static offset (collider centre offset in the bone's
/// rest pose, in world units).
#[derive(Component, Debug, Default, Clone)]
pub struct PhysicsColliderStaticOffsets(
    #[expect(dead_code, reason = "yet to be wired to update_bone_positions")] pub Vec<Vec3>,
);

/// Per-bone static rotation (collider local rotation baked from
/// the rest pose).
#[derive(Component, Debug, Default, Clone)]
pub struct PhysicsColliderStaticRotations(
    #[expect(dead_code, reason = "yet to be wired to update_bone_positions")] pub Vec<Quat>,
);

/// Per-bone rest world rotation (used to compute `r_delta`).
#[derive(Component, Debug, Default, Clone)]
pub struct PhysicsColliderRestRotations(
    #[expect(dead_code, reason = "yet to be wired to update_bone_positions")] pub Vec<Quat>,
);
