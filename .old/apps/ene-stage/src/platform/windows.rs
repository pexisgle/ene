//! Windows overlay hit-test: window-wide `set_cursor_hittest` only.

use winit::window::Window;

use crate::interaction_controller::InteractionMode;

pub fn apply_mode(window: &Window, mode: InteractionMode) {
    let enabled = !matches!(mode, InteractionMode::Passive);
    tracing::debug!(?mode, enabled, "windows overlay interaction mode");
    match window.set_cursor_hittest(enabled) {
        Ok(()) => {}
        Err(err) => tracing::debug!(error = %err, "set_cursor_hittest failed"),
    }
}
