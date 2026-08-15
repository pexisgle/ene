//! Each plugin owns the resources, components, messages, and
//! systems for one slice of the application. They are added to the
//! `App` in [`crate::app::CorePlugin::build`].
//!
//! The render path stays on the `Runtime`: `CharacterRenderer` and
//! `wgpu::Device` / `wgpu::Queue` are `!Send + !Sync`, so the
//! per-frame `character.update_*` / `character.render` /
//! `cw.with_surface_view` calls cannot live in bevy systems.
pub mod ai_plugin;
pub mod character_plugin;
pub mod chat_plugin;
pub mod physics_plugin;
pub mod platform_plugin;
pub mod tray_plugin;
pub mod ui_plugin;
