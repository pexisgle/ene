//! # ECS Systems for `ene-desktop`
//!
//! Each system is a small, focused function that reads [`Res<T>`] /
//! [`ResMut<T>`] and / or [`Query`] / [`MessageReader<T>`] parameters
//! and writes back. The schedule in [`crate::schedule`] controls the
//! execution order.
//!
//! ## Phase 6 scope
//!
//! | Stage        | System                              |
//! |--------------|-------------------------------------|
//! | `First`      | `event_pump::pump_legacy_events`    |
//! | `Startup`    | `physics::attach_bone_colliders_system` |
//! | `Update`     | `physics::step_physics_system`      |
//! | `Update`     | `platform::cursor::update_cursor_state_system` |
//! | `Update`     | `platform::click_through::apply_linux_click_through_system` |
//! | `Update`     | `platform::input_region::refresh_input_region_system` |
//! | `Update`     | `platform::gtk_pump::tick_gtk_system` |
//! | `Last`       | `tray_tick::tick_gtk_system` (Linux) |
//! | `Update`     | `ui_consumers::open_settings_system` |
//! | `Update`     | `ui_consumers::apply_ai_text_deltas_system` |
//! | `Update`     | `ui_consumers::apply_ai_permission_system` |
//! | `Update`     | `ui_consumers::apply_ai_user_input_system` |
pub mod event_pump;
pub mod physics;
pub mod platform;
pub mod tray_tick;
pub mod ui_consumers;
pub mod ui_dispatcher;
