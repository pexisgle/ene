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
//! - [`emotion_pipeline::EmotionPipelineState`] — drained by
//!   `apply_emotions_system` and read by the render path.
//! - [`tray::TrayHandleResource`] — Linux-only `NonSend`-friendly
//!   wrapper around the tray handle, consumed by
//!   `tick_gtk_system`.
//! - [`tokio::TokioHandle`] — the tokio runtime handle used to spawn
//!   async subsystems (AI bridge, tray pump).
//!
//! Phase 7.2-final note: the planned `gpu_resources` / `render_frame`
//! resources were deleted in Phase 8 cleanup. The per-frame
//! render parameters stay on `Runtime` because they are GPU-
//! and `!Send`/`!Sync`-bound; see `docs/reference/architecture/startup.md`.
pub mod ai_bridge;
pub mod cursor_state;
pub mod emotion_pipeline;
pub mod event_channels;
pub mod exit;
pub mod frame_state;
pub mod motion_layer;
pub mod physics;
pub mod platform_state;
pub mod tokio;
pub mod tray;
