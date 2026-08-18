//! # ECS Resources for `ene-desktop`
//!
//! This module holds the singleton resources that the [`App`](bevy_app::App)
//! world owns. Global state that systems need lives in a separate
//! resource here so they can pull it via [`Res<T>`] / [`ResMut<T>`]
//! instead of reaching through `AppState` fields.
//!
//! The new resources coexist with `AppState`: `AppState` keeps a
//! reference to the same `World` and its fields act as cached
//! read-throughs (see [`crate::state::AppState`]).
//!
//! ## Layout
//!
//! - [`frame_state::FrameState`] — per-frame timing data.
//! - [`exit::ExitRequested`] — flag set when the OS requests shutdown.
//! - [`event_channels::EventChannels`] — sender half of the `AppEvent`
//!   bus plus the receiver. Drained by the `pump_legacy_events` system
//!   in `First`.
//! - [`emotion_pipeline::EmotionPipelineState`] — drained by
//!   `apply_emotions_system` and read by the render path.
//! - [`tray::TrayHandleResource`] — Linux-only `NonSend`-friendly
//!   wrapper around the tray handle, consumed by `tick_gtk_system`.
//! - [`tokio::TokioHandle`] — the tokio runtime handle used to spawn
//!   async subsystems (AI bridge, tray pump).
//!
//! Per-frame render parameters stay on `Runtime`, not in resources,
//! because they are GPU- and `!Send`/`!Sync`-bound.
pub mod ai_bridge;
pub mod beat_sync;
pub mod cursor_state;
pub mod emotion_pipeline;
pub mod event_channels;
pub mod exit;
pub mod frame_state;
pub mod motion_layer;
pub mod physics;
pub mod platform_state;
pub mod startup;
pub mod tokio;
pub mod tray;
