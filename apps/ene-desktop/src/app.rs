//! # Application root and plugin set
//!
//! The new ECS architecture is composed entirely of small [`Plugin`]
//! implementations, each owning a slice of resources, components,
//! messages, and systems. This module just glues them together and exposes
//! a single [`build_app`] entry point used by `main.rs`.
//!
//! ## Phased migration
//!
//! During the refactor the legacy `AppState` and the new
//! `bevy_ecs` `World` coexist. Each plugin below is added once the
//! corresponding legacy code has been retired; the placeholder comments
//! inside [`CorePlugin`] reserve a slot in the plugin set so the
//! [`build_app`] ordering is stable across phases.
use bevy_app::{App, Plugin, PluginGroup, PluginGroupBuilder};

use crate::event::chat::OpenChat;
use crate::event::{
    ai::{
        AiPermissionRequested, AiStreamError, AiStreamFinished, AiTextDelta, AiUserInputRequested,
        CancelCommand, EmoteToken, ExpressionCommand, MotionCommand,
    },
    input::{KeyboardKey, PointerButton, PointerMoved},
    lifecycle::{TickGtk, WindowCloseRequested, WindowResized},
    settings::OpenSettings,
    ui_action::SettingsActionEvent,
};
use crate::events::AppEventReceiver;
use crate::plugin::ai_plugin::AiPlugin;
use crate::plugin::character_plugin::CharacterPlugin;
use crate::plugin::chat_plugin::ChatPlugin;
use crate::plugin::physics_plugin::PhysicsPlugin;
use crate::plugin::platform_plugin::PlatformPlugin;
use crate::plugin::tray_plugin::TrayPlugin;
use crate::plugin::ui_plugin::UiPlugin;
use crate::resource::{
    emotion_pipeline::EmotionPipelineState, event_channels::EventChannels, exit::ExitRequested,
    frame_state::FrameState, motion_layer::MotionLayerState, tokio::TokioHandle,
};
use crate::schedule::{configure_schedule, configure_startup};

/// The full plugin set for `ene-desktop`.
///
/// Plugin order matters only for the `Startup` schedule; the per-frame
/// `First` / `PreUpdate` / `Update` / `PostUpdate` / `Last` stages sort
/// themselves by [`IntoScheduleConfigs`] constraints.
#[derive(Default)]
pub struct DesktopPlugins;

impl PluginGroup for DesktopPlugins {
    fn build(self) -> PluginGroupBuilder {
        PluginGroupBuilder::start::<Self>()
            .add(CorePlugin)
            .add(CharacterPlugin)
            .add(PhysicsPlugin)
            .add(UiPlugin)
            .add(ChatPlugin)
            .add(PlatformPlugin)
            .add(TrayPlugin)
            .add(AiPlugin)
    }
}

/// Marks the bare minimum needed for the schedule to be valid: the
/// `Startup` / `First` / `PreUpdate` / `Update` / `PostUpdate` / `Last`
/// stages, the empty [`crate::schedule::AppSet`] marker slots, the
/// Phase 1 singleton resources, and the Phase 2 [`Message`] queue
/// registrations.
#[derive(Default)]
pub struct CorePlugin;

impl Plugin for CorePlugin {
    fn build(&self, app: &mut App) {
        configure_startup(app);
        configure_schedule(app);
    }
}

/// Builds and returns a fully configured [`App`] with all Phase 1
/// resources, Phase 2 message queues, and the legacy `EventChannels`
/// resource inserted. The returned `App` is ready to have its
/// schedule run from the winit event loop via [`App::update`].
pub fn build_app(
    tokio: tokio::runtime::Handle,
    event_rx: AppEventReceiver,
    event_tx: crate::events::AppEventSender,
) -> App {
    let mut app = App::new();
    app.add_plugins(DesktopPlugins);
    app.insert_resource(FrameState::default());
    app.insert_resource(ExitRequested::default());
    app.insert_resource(TokioHandle(tokio));
    app.insert_resource(EmotionPipelineState::default());
    app.insert_resource(MotionLayerState::default());
    app.insert_resource(EventChannels {
        tx: event_tx,
        rx: event_rx,
    });
    app.add_message::<PointerMoved>();
    app.add_message::<PointerButton>();
    app.add_message::<KeyboardKey>();
    app.add_message::<WindowResized>();
    app.add_message::<WindowCloseRequested>();
    app.add_message::<AiTextDelta>();
    app.add_message::<AiStreamFinished>();
    app.add_message::<AiStreamError>();
    app.add_message::<AiPermissionRequested>();
    app.add_message::<AiUserInputRequested>();
    app.add_message::<EmoteToken>();
    app.add_message::<MotionCommand>();
    app.add_message::<ExpressionCommand>();
    app.add_message::<CancelCommand>();
    app.add_message::<OpenSettings>();
    app.add_message::<OpenChat>();
    app.add_message::<SettingsActionEvent>();
    app.add_message::<TickGtk>();
    app
}

#[cfg(test)]
mod message_registration {
    use super::*;
    use crate::events::AppEvent;
    use bevy_ecs::message::Messages;
    use tokio::sync::mpsc;

    #[test]
    fn expression_and_cancel_messages_are_registered() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("test runtime builds");
        let (tx, rx) = mpsc::unbounded_channel::<AppEvent>();
        let app = build_app(rt.handle().clone(), rx, tx);
        let world = app.world();
        assert!(
            world.contains_resource::<Messages<ExpressionCommand>>(),
            "ExpressionCommand must be registered via add_message"
        );
        assert!(
            world.contains_resource::<Messages<CancelCommand>>(),
            "CancelCommand must be registered via add_message"
        );
    }
}
