//! Set by winit's `ApplicationHandler::about_to_wait` after observing
//! a `WindowEvent::CloseRequested` or an [`crate::events::AppEvent::Quit`].
//! Consumed by the runtime to call `event_loop.exit()`.
use bevy_ecs::prelude::*;

/// Boolean flag flipped by systems to signal the winit runtime that
/// the application should terminate at the end of the current frame.
#[derive(Resource, Debug, Default, Clone, Copy)]
pub struct ExitRequested(pub bool);
