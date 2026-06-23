//! Scene graph transforms.
//!
//! Minimal `Transform` / `GlobalTransform` pair used by the
//! `propagate_transforms` system (added in Phase 4 alongside
//! physics). Kept as plain-data components so the entity can be
//! moved without reaching for the legacy `physics::Transform`.
use bevy_ecs::prelude::*;
use glam::{Mat4, Quat, Vec3};

#[derive(Component, Debug, Default, Clone, Copy)]
pub struct Transform {
    #[expect(dead_code, reason = "Read by propagate_transforms in Phase 4")]
    pub translation: Vec3,
    #[expect(dead_code, reason = "Read by propagate_transforms in Phase 4")]
    pub rotation: Quat,
    #[expect(dead_code, reason = "Read by propagate_transforms in Phase 4")]
    pub scale: Vec3,
}

#[derive(Component, Debug, Default, Clone, Copy)]
pub struct GlobalTransform(
    #[expect(dead_code, reason = "Read by propagate_transforms in Phase 4")] pub Mat4,
);
