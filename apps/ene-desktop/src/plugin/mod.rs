//! # Plugin set for `ene-desktop`
//!
//! Each plugin owns the resources, components, messages, and
//! systems for one slice of the application. They are added to the
//! `App` in [`crate::app::CorePlugin::build`].
//!
//! ## Phase 5 status
//!
//! [`CharacterPlugin`], [`PhysicsPlugin`], and [`UiPlugin`] are
//! registered; the other plugins are placeholders for later
//! phases:
//!
//! - `WindowPlugin` (Phase 7)
//! - `RenderPlugin` (Phase 7)
//! - `SettingsPlugin` (Phase 5+)
//! - `TrayPlugin` (Phase 6)
//! - `PlatformPlugin` (Phase 6)
//! - `AiPlugin` (Phase 6)
//! - `DebugPlugin` (Phase 8)
pub mod character_plugin;
pub mod physics_plugin;
pub mod ui_plugin;
