//! ECS components for the dedicated chat window (#109).

use bevy_ecs::prelude::*;

use crate::chat_state::ChatState;

/// Marker on the single chat-window entity.
#[derive(Component, Default)]
pub struct ChatWindow;

/// Runtime chat UI state mirrored by consumer systems and the egui
/// render path.
#[derive(Component, Default)]
pub struct ChatStateComponent(pub ChatState);

/// Bundle spawned by [`crate::plugin::chat_plugin::spawn_chat_window`].
#[derive(Bundle, Default)]
pub struct ChatUiBundle {
    pub window: ChatWindow,
    pub state: ChatStateComponent,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundle_defaults_are_safe() {
        let bundle = ChatUiBundle::default();
        assert!(bundle.state.0.messages.is_empty());
        assert!(bundle.state.0.chat_window_visible);
    }
}
