//! # ECS Systems for `ene-desktop`
//!
//! Each system is a small, focused function that reads [`Res<T>`] /
//! [`ResMut<T>`] and / or [`Query`] / [`MessageReader<T>`] parameters
//! and writes back. The schedule in [`crate::schedule`] controls the
//! execution order.
pub mod event_pump;
pub mod physics;
pub mod platform;
pub mod tray_tick;
pub mod ui_consumers;
pub mod ui_dispatcher;
