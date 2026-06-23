//! # ECS Systems for `ene-desktop`
//!
//! Each system is a small, focused function that reads [`Res<T>`] /
//! [`ResMut<T>`] and / or [`Query`] / [`MessageReader<T>`] parameters
//! and writes back. The schedule in [`crate::schedule`] controls the
//! execution order.
//!
//! ## Phase 2 scope
//!
//! | Stage        | System                              |
//! |--------------|-------------------------------------|
//! | `First`      | `event_pump::pump_legacy_events`    |
//! | `Startup`    | `physics::attach_bone_colliders_system` |
//! | `Update`     | `physics::step_physics_system`      |
pub mod event_pump;
pub mod physics;
