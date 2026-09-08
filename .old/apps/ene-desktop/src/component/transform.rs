//! Minimal `Transform` / `GlobalTransform` pair used by the
//! `propagate_transforms` system. Kept as plain-data components so the
//! entity can be moved without reaching for `physics::Transform`.
use bevy_ecs::prelude::*;
use glam::{Mat4, Quat, Vec3};

#[derive(Component, Debug, Default, Clone, Copy)]
pub struct Transform {
    #[expect(dead_code, reason = "yet to be wired to propagate_transforms")]
    pub translation: Vec3,
    #[expect(dead_code, reason = "yet to be wired to propagate_transforms")]
    pub rotation: Quat,
    #[expect(dead_code, reason = "yet to be wired to propagate_transforms")]
    pub scale: Vec3,
}

#[derive(Component, Debug, Default, Clone, Copy)]
pub struct GlobalTransform(
    #[expect(dead_code, reason = "yet to be wired to propagate_transforms")] pub Mat4,
);
