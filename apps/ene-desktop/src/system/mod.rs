//! Each system is a small, focused function that reads bevy `Res` /
//! bevy `ResMut` / `Query` / `MessageReader` parameters
//! and writes back. The schedule module controls the
//! execution order.
pub mod event_pump;
pub mod physics;
pub mod platform;
pub mod ui_consumers;
pub mod ui_dispatcher;
