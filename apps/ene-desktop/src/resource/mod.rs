//! # ECS Resources for `ene-desktop`
//!
//! This module holds the singleton resources that the [`App`](bevy_app::App)
//! world owns. Every global piece of state from the old
//! [`AppState`](crate::state::AppState) is migrated into a separate
//! resource here so that systems can pull them via [`Res<T>`] / [`ResMut<T>`]
//! instead of reaching through nested `Mutex<AppState>` fields.
//!
//! ## Phased migration
//!
//! During the refactor the new resources coexist with the legacy
//! [`AppState`](crate::state::AppState) — `AppState` keeps a reference
//! to the same `World` and its fields act as cached read-throughs
//! (see [`crate::state::AppState`]).
//!
//! ## Layout
//!
//! - [`frame_state::FrameState`] — per-frame timing data.
//! - [`exit::ExitRequested`] — flag set when the OS requests shutdown.
//! - [`event_channels::EventChannels`] — sender half of the legacy
//!   `AppEvent` bus plus the receiver. Drained by the
//!   `pump_legacy_events` system in `First`.
//! - [`pending_actions::PendingActions`] — per-frame buffer written
//!   by the pump system and read by the legacy `Runtime` body.
//! - [`tokio::TokioHandle`] — the tokio runtime handle used to spawn
//!   async subsystems (AI bridge, tray pump).
pub mod event_channels;
pub mod exit;
pub mod frame_state;
pub mod pending_actions;
pub mod physics;
pub mod tokio;
