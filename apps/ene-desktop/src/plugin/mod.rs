//! # Plugin set for `ene-desktop`
//!
//! Each plugin owns the resources, components, messages, and
//! systems for one slice of the application. They are added to the
//! `App` in [`crate::app::CorePlugin::build`].
//!
//! ## Phase 6 status
//!
//! [`CharacterPlugin`], [`PhysicsPlugin`], [`UiPlugin`],
//! [`PlatformPlugin`], [`TrayPlugin`], and [`AiPlugin`] are
//! registered. The render path itself stays in the runtime:
//! `CharacterRenderer` and `wgpu::Device` / `wgpu::Queue` are
//! `!Send + !Sync`, so the per-frame `character.update_*` /
//! `character.render` / `cw.with_surface_view` calls cannot
//! live in bevy systems. The render path stays on `Runtime` for that
//! reason; see `docs/reference/architecture/startup.md` and
//! `docs/guide/apps/desktop.md`.
//!
//! - `WindowPlugin` (deferred)
//! - `RenderPlugin` (cancelled — GPU work stays on `Runtime`)
//! - `DebugPlugin` (deferred)
pub mod ai_plugin;
pub mod character_plugin;
pub mod chat_plugin;
pub mod physics_plugin;
pub mod platform_plugin;
pub mod tray_plugin;
pub mod ui_plugin;
