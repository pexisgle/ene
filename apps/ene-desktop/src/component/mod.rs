//! # ECS Components for `ene-desktop`
//!
//! Each component holds a small piece of plain data attached to an
//! entity. The full set is split per concern:
//!
//! - [`character`] — components on the primary character entity:
//!   VRM model handle, motion / spring-bone state, camera, look-at,
//!   emotion channel, drag state, bone colliders.
//! - [`transform`] — `Transform` and `GlobalTransform` for scene
//!   graph propagation (added in Phase 4 alongside physics).
//! - [`ui`] — UI window, page, input drafts (added in Phase 5).
//! - [`physics`] — rigid-body / collider handles (added in Phase 4).
pub mod character;
pub mod chat;
pub mod physics;
pub mod transform;
pub mod ui;
