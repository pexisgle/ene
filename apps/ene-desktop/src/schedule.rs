//! # ECS Schedule for `ene-desktop`
//!
//! This module defines the [`AppSet`] system sets and the [`configure_schedule`]
//! helper that powers the desktop application. The 5-stage layout follows
//! the [`bevy_app`] convention (with a `Startup` prelude for one-shot
//! initialization), giving us a deterministic and idiomatic execution order
//! for every frame of the runtime.
//!
//! ## Why `bevy_ecs` instead of `hecs`
//!
//! The codebase previously embedded a single `hecs::World` to hold a
//! [`Transform`](crate::character::Transform) and a
//! [`UiState`](crate::settings::UiState) component, but the bulk of the
//! per-frame logic lived in a single `Runtime::about_to_wait` method.
//! `bevy_ecs` brings a `Schedule` / `SystemSet` / `Message` / `Resource`
//! toolbox that lets us express that logic as small, composable systems.
//!
//! ## Stages
//!
//! | Stage          | Purpose                                                          |
//! |----------------|------------------------------------------------------------------|
//! | `Startup`      | One-shot initialization (resources, egui, VRM loading).          |
//! | `First`        | Pump external event sources (winit, AI bridge, tray).            |
//! | `PreUpdate`    | Translate raw events into ECS messages; run structural changes.  |
//! | `Update`       | Game / app logic (input, animation, settings, physics).          |
//! | `PostUpdate`   | Mirror ECS state into GPU staging buffers.                       |
//! | `Last`         | Acquire the surface, encode passes, submit, present.             |
//!
//! Systems that must touch non-`Send` resources (e.g. `wgpu::Surface`,
//! `egui::Context`) use [`NonSendMut`](bevy_ecs::system::NonSendMut) so the
//! executor keeps them on the main thread.
use bevy_app::{App, First, Last, PostUpdate, PreUpdate, Startup, Update};
use bevy_ecs::prelude::*;

use crate::system::event_pump::pump_legacy_events;

/// Per-stage ordering labels used inside the five standard `bevy_app` stages.
///
/// These are *not* new schedules; they are markers that any system can be
/// pinned to within `First` / `PreUpdate` / `Update` / `PostUpdate` / `Last`.
/// The full ordering of systems inside a stage is still controlled with
/// [`IntoScheduleConfigs::chain`], [`IntoScheduleConfigs::before`] and
/// [`IntoScheduleConfigs::after`].
#[derive(SystemSet, Debug, Hash, PartialEq, Eq, Clone, Copy)]
pub enum AppSet {
    /// Convert polled raw events into ECS [`Message`]s.
    EventDispatch,
    /// Input handling (pointer, drag, look-at).
    Input,
    /// Apply user-driven settings changes.
    Settings,
    /// Per-frame logic: animation, emotion, spring bones, physics.
    Animation,
    /// Build GPU staging data from current ECS state.
    Render,
    /// Submit, present, and apply platform-specific post-frame work.
    Present,
}

/// Registers the per-stage ordering of the [`AppSet`] markers and
/// Phase 2 pump systems. The schedule runs once per frame via
/// [`App::update`] from `Runtime::about_to_wait`.
pub fn configure_schedule(app: &mut App) {
    app.configure_sets(First, AppSet::EventDispatch);
    app.configure_sets(PreUpdate, AppSet::EventDispatch);
    app.configure_sets(
        Update,
        (AppSet::Input, AppSet::Settings, AppSet::Animation).chain(),
    );
    app.configure_sets(PostUpdate, AppSet::Render);
    app.configure_sets(Last, (AppSet::Render, AppSet::Present).chain());

    // First: drain the legacy `AppEvent` bus and emit typed Messages.
    app.add_systems(First, pump_legacy_events.in_set(AppSet::EventDispatch));
}

/// `Startup` schedule marker: all one-shot setup systems are added here.
pub fn configure_startup(app: &mut App) {
    app.configure_sets(Startup, AppSet::EventDispatch);
}
