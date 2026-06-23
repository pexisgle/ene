//! # ECS Messages for `ene-desktop`
//!
//! Each module re-exports [`Message`](bevy_ecs::message::Message) types
//! that the `First`-stage pump system emits and `Update` / `Last`
//! consumer systems read.
pub mod ai;
pub mod input;
pub mod lifecycle;
pub mod settings;
pub mod ui_action;
