//! # Application root and plugin set
//!
//! The new ECS architecture is composed entirely of small [`Plugin`]
//! implementations, each owning a slice of resources, components,
//! messages, and systems. This module just glues them together and exposes
//! a single [`build_app`] entry point used by `main.rs`.
use bevy_app::{App, Plugin, PluginGroup, PluginGroupBuilder, Update};

use crate::event::chat::OpenChat;
use crate::event::{
    ai::{
        AiPermissionRequested, AiStreamError, AiStreamFinished, AiTextDelta, AiUserInputRequested,
        BeatPulse, CancelCommand, EmoteToken, ExpressionCommand, MotionCommand,
        PendingCandidatesCount,
    },
    lifecycle::WindowCloseRequested,
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
/// themselves by bevy `IntoScheduleConfigs` constraints.
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
/// standard `bevy_app` stages and the [`crate::schedule::AppSet`]
/// marker slots.
#[derive(Default)]
pub struct CorePlugin;

impl Plugin for CorePlugin {
    fn build(&self, app: &mut App) {
        configure_startup(app);
        configure_schedule(app);
    }
}

/// The returned `App` is ready to have its schedule run from the winit event
/// loop via [`App::update`].
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
    app.init_resource::<crate::caption_overlay::CaptionFeed>();
    app.insert_resource(crate::resource::beat_sync::BeatSyncState::default());
    app.insert_resource(EventChannels {
        tx: event_tx,
        rx: event_rx,
        detail_requested: false,
    });
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
    app.add_message::<BeatPulse>();
    app.add_message::<OpenSettings>();
    app.add_message::<OpenChat>();
    app.add_message::<SettingsActionEvent>();
    app.add_message::<PendingCandidatesCount>();
    app.add_message::<crate::event::lifecycle::RuntimeDisconnected>();
    app.add_systems(Update, crate::caption_overlay::feed_caption_overlay_system);
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
