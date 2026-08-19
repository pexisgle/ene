//! Systems that drain the `AppEvent` bus and translate it
//! into typed `bevy_ecs::message::Message`s.
//!
//! The pump runs in the `First` stage. Messages written by the pump
//! are consumed by `Update` stage systems.
use std::time::Instant;

use bevy_ecs::prelude::*;

use crate::event::ai::{
    AiPermissionRequested, AiStreamError, AiStreamFinished, AiTextDelta, AiUserInputRequested,
    BeatPulse, CancelCommand, EmoteToken, ExpressionCommand, MotionCommand, PendingCandidatesCount,
};
use crate::event::chat::OpenChat;
#[cfg(target_os = "linux")]
use crate::event::lifecycle::TickGtk;
use crate::event::settings::OpenSettings;
use crate::events::{AiStreamUpdate, AppEvent};
use crate::resource::{event_channels::EventChannels, exit::ExitRequested};

/// Runs in the `First` stage so any `Update` systems see fresh events.
///
/// This pump is the single producer of every typed `Message` the
/// desktop runtime consumes; per-frame actions are handled by the
/// consumer systems in `system::ui_consumers.rs` reading the
/// `Message` queue.
pub fn pump_legacy_events(
    mut channels: ResMut<EventChannels>,
    mut exit: ResMut<ExitRequested>,
    mut text_delta: MessageWriter<AiTextDelta>,
    mut permission: MessageWriter<AiPermissionRequested>,
    mut user_input: MessageWriter<AiUserInputRequested>,
    mut finished: MessageWriter<AiStreamFinished>,
    mut stream_error: MessageWriter<AiStreamError>,
    mut emote: MessageWriter<EmoteToken>,
    mut motion: MessageWriter<MotionCommand>,
    mut expression: MessageWriter<ExpressionCommand>,
    mut cancel: MessageWriter<CancelCommand>,
    mut beat_pulse: MessageWriter<BeatPulse>,
    mut open_settings: MessageWriter<OpenSettings>,
    mut open_chat: MessageWriter<OpenChat>,
    mut runtime_disconnected: MessageWriter<crate::event::lifecycle::RuntimeDisconnected>,
    mut pending_candidates: MessageWriter<PendingCandidatesCount>,
) {
    while let Ok(event) = channels.rx.try_recv() {
        if matches!(event, AppEvent::Tray(crate::events::TrayAction::OpenDetail)) {
            channels.detail_requested = true;
            continue;
        }
        translate_event(
            event,
            &mut exit,
            &mut text_delta,
            &mut permission,
            &mut user_input,
            &mut finished,
            &mut stream_error,
            &mut emote,
            &mut motion,
            &mut expression,
            &mut cancel,
            &mut beat_pulse,
            &mut open_settings,
            &mut open_chat,
            &mut runtime_disconnected,
            &mut pending_candidates,
        );
    }
}

/// Publish a `TickGtk` every frame on Linux so the tray icon library makes
/// progress. Split from [`pump_legacy_events`] to keep that system within
/// bevy's 16-parameter limit. The actual `tick_gtk()` call still lives in
/// `Runtime::about_to_wait` because the `Rc<RefCell<TrayHandle>>` is not
/// `Send + Sync`.
#[cfg(target_os = "linux")]
pub fn publish_tick_gtk_system(mut tick_gtk: MessageWriter<TickGtk>) {
    tick_gtk.write(TickGtk);
}

fn translate_event(
    event: AppEvent,
    exit: &mut ExitRequested,
    text_delta: &mut MessageWriter<AiTextDelta>,
    permission: &mut MessageWriter<AiPermissionRequested>,
    user_input: &mut MessageWriter<AiUserInputRequested>,
    finished: &mut MessageWriter<AiStreamFinished>,
    stream_error: &mut MessageWriter<AiStreamError>,
    emote: &mut MessageWriter<EmoteToken>,
    motion: &mut MessageWriter<MotionCommand>,
    expression: &mut MessageWriter<ExpressionCommand>,
    cancel: &mut MessageWriter<CancelCommand>,
    beat_pulse: &mut MessageWriter<BeatPulse>,
    open_settings: &mut MessageWriter<OpenSettings>,
    open_chat: &mut MessageWriter<OpenChat>,
    runtime_disconnected: &mut MessageWriter<crate::event::lifecycle::RuntimeDisconnected>,
    pending_candidates: &mut MessageWriter<PendingCandidatesCount>,
) {
    match event {
        AppEvent::Quit | AppEvent::Tray(crate::events::TrayAction::Quit) => {
            exit.0 = true;
        }
        AppEvent::RuntimeDisconnected => {
            runtime_disconnected.write(crate::event::lifecycle::RuntimeDisconnected);
        }
        AppEvent::Tray(crate::events::TrayAction::OpenSettings { page }) => {
            open_settings.write(OpenSettings { page });
        }
        AppEvent::Tray(crate::events::TrayAction::OpenChat) => {
            open_chat.write(OpenChat);
        }
        AppEvent::Ai(AiStreamUpdate::TextDelta(text)) => {
            text_delta.write(AiTextDelta(text));
        }
        AppEvent::Ai(AiStreamUpdate::Finished) => {
            finished.write(AiStreamFinished);
        }
        AppEvent::Ai(AiStreamUpdate::Error(message)) => {
            stream_error.write(AiStreamError(message));
        }
        AppEvent::Ai(AiStreamUpdate::PermissionRequired {
            request_id,
            action,
            target,
            description,
        }) => {
            permission.write(AiPermissionRequested {
                request_id,
                action,
                target,
                description,
            });
        }
        AppEvent::Ai(AiStreamUpdate::UserInputRequired { request_id, prompt }) => {
            user_input.write(AiUserInputRequested { request_id, prompt });
        }
        AppEvent::PerformanceCue(name) => {
            emote.write(EmoteToken(name));
        }
        AppEvent::ExpressionCue {
            name,
            weight,
            hold_secs,
            target_time,
        } => {
            expression.write(ExpressionCommand {
                name,
                weight,
                hold_secs,
                target_time,
            });
        }
        AppEvent::CancelCue { scope } => {
            cancel.write(CancelCommand(scope));
        }
        AppEvent::MotionCue {
            name,
            layer,
            priority,
            duration,
        } => {
            motion.write(MotionCommand {
                name,
                layer,
                priority,
                duration,
            });
        }
        AppEvent::BeatPulse { bpm, intensity } => {
            beat_pulse.write(BeatPulse { bpm, intensity });
        }
        AppEvent::Ai(
            AiStreamUpdate::ToolCallStart { .. } | AiStreamUpdate::ToolCallResult { .. },
        ) => {}
        AppEvent::LookAtCue { ref target } => {
            tracing::debug!(
                component = "CoreSession",
                target = %target,
                "LookAt cue received (gaze system pending)"
            );
        }
        #[cfg(feature = "voice")]
        AppEvent::MicStateChanged { active } => {
            // The chat UI reads `AudioState::is_mic_active` directly each
            // frame (egui polls continuously); this event exists so future
            // consumers (e.g. a tray indicator) can react to mic toggles.
            tracing::debug!(
                component = "Audio",
                active,
                "microphone capture state changed"
            );
        }
        AppEvent::PendingCandidatesCount(count) => {
            pending_candidates.write(PendingCandidatesCount(count));
        }
        AppEvent::Tray(crate::events::TrayAction::OpenDetail) => {}
    }
}

/// No-op retained for API symmetry; the GTK tick now flows through the
/// `Messages<TickGtk>` queue consumed by `tick_gtk_system`.
#[expect(
    dead_code,
    reason = "Replaced by Messages<TickGtk>; kept for API symmetry"
)]
pub const fn mark_gtk_tick() {}

/// Suppress unused-imports for the `Instant` import in non-tray builds.
const fn _force_link(_: Instant) {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::TrayAction;
    use crate::settings_ui::PageKind;
    use bevy_ecs::message::MessageReader;
    use tokio::sync::mpsc;

    fn build_world() -> (World, mpsc::UnboundedSender<AppEvent>) {
        let (tx, rx) = mpsc::unbounded_channel::<AppEvent>();
        let mut world = World::new();
        world.insert_resource(EventChannels {
            tx: tx.clone(),
            rx,
            detail_requested: false,
        });
        world.insert_resource(ExitRequested::default());
        world.init_resource::<Messages<AiTextDelta>>();
        world.init_resource::<Messages<AiStreamFinished>>();
        world.init_resource::<Messages<AiStreamError>>();
        world.init_resource::<Messages<AiPermissionRequested>>();
        world.init_resource::<Messages<AiUserInputRequested>>();
        world.init_resource::<Messages<EmoteToken>>();
        world.init_resource::<Messages<MotionCommand>>();
        world.init_resource::<Messages<ExpressionCommand>>();
        world.init_resource::<Messages<CancelCommand>>();
        world.init_resource::<Messages<BeatPulse>>();
        world.init_resource::<Messages<OpenSettings>>();
        world.init_resource::<Messages<OpenChat>>();
        world.init_resource::<Messages<crate::event::lifecycle::RuntimeDisconnected>>();
        world.init_resource::<Messages<PendingCandidatesCount>>();
        #[cfg(target_os = "linux")]
        world.init_resource::<Messages<crate::event::lifecycle::TickGtk>>();
        (world, tx)
    }

    fn run_pump(world: &mut World) {
        let mut schedule = Schedule::default();
        schedule.add_systems(pump_legacy_events);
        schedule.run(world);
    }

    #[test]
    fn quit_event_sets_exit() {
        let (mut world, tx) = build_world();
        tx.send(AppEvent::Quit).unwrap();
        run_pump(&mut world);
        assert!(world.resource::<ExitRequested>().0);
    }

    #[test]
    fn tray_quit_sets_exit() {
        let (mut world, tx) = build_world();
        tx.send(AppEvent::Tray(TrayAction::Quit)).unwrap();
        run_pump(&mut world);
        assert!(world.resource::<ExitRequested>().0);
    }

    #[test]
    fn open_settings_emits_typed_message() {
        let (mut world, tx) = build_world();
        tx.send(AppEvent::Tray(TrayAction::OpenSettings {
            page: Some(PageKind::Ai),
        }))
        .unwrap();
        run_pump(&mut world);
        let messages = world.resource_mut::<Messages<OpenSettings>>();
        let mut cursor = messages.get_cursor();
        let events: Vec<_> = cursor.read(&*messages).collect();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].page, Some(PageKind::Ai));
    }

    #[test]
    fn permission_required_emits_typed_message() {
        let (mut world, tx) = build_world();
        tx.send(AppEvent::Ai(AiStreamUpdate::PermissionRequired {
            request_id: "req-1".to_string(),
            action: "read".to_string(),
            target: "file.txt".to_string(),
            description: "needs approval".to_string(),
        }))
        .unwrap();
        run_pump(&mut world);
        let messages = world.resource_mut::<Messages<AiPermissionRequested>>();
        let mut cursor = messages.get_cursor();
        let events: Vec<_> = cursor.read(&*messages).collect();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].action, "read");
        assert_eq!(events[0].target, "file.txt");
        assert_eq!(events[0].description, "needs approval");
    }

    #[test]
    fn text_delta_emits_typed_messages() {
        let (mut world, tx) = build_world();
        tx.send(AppEvent::Ai(AiStreamUpdate::TextDelta(
            "hello ".to_string(),
        )))
        .unwrap();
        tx.send(AppEvent::Ai(AiStreamUpdate::TextDelta("world".to_string())))
            .unwrap();
        run_pump(&mut world);
        let messages = world.resource_mut::<Messages<AiTextDelta>>();
        let mut cursor = messages.get_cursor();
        let events: Vec<_> = cursor.read(&*messages).collect();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].0, "hello ");
        assert_eq!(events[1].0, "world");
    }

    #[test]
    fn emote_token_emits_typed_message() {
        let (mut world, tx) = build_world();
        tx.send(AppEvent::PerformanceCue("happy".to_string()))
            .unwrap();
        run_pump(&mut world);
        let messages = world.resource_mut::<Messages<EmoteToken>>();
        let mut cursor = messages.get_cursor();
        let events: Vec<_> = cursor.read(&*messages).collect();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].0, "happy");
    }

    #[test]
    #[expect(clippy::float_cmp, reason = "test asserts exact float equality")]
    fn expression_cue_emits_typed_message() {
        let (mut world, tx) = build_world();
        tx.send(AppEvent::ExpressionCue {
            name: "happy".to_string(),
            weight: 0.8,
            hold_secs: 3.0,
            target_time: 2.5,
        })
        .unwrap();
        run_pump(&mut world);
        let messages = world.resource_mut::<Messages<ExpressionCommand>>();
        let mut cursor = messages.get_cursor();
        let events: Vec<_> = cursor.read(&*messages).collect();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].name, "happy");
        assert_eq!(events[0].target_time, 2.5);
    }

    #[test]
    fn cancel_cue_emits_typed_message() {
        let (mut world, tx) = build_world();
        tx.send(AppEvent::CancelCue {
            scope: "expr".to_string(),
        })
        .unwrap();
        run_pump(&mut world);
        let messages = world.resource_mut::<Messages<CancelCommand>>();
        let mut cursor = messages.get_cursor();
        let events: Vec<_> = cursor.read(&*messages).collect();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].0, "expr");
    }

    #[test]
    #[expect(clippy::float_cmp, reason = "test asserts exact float equality")]
    fn beat_pulse_emits_typed_message() {
        let (mut world, tx) = build_world();
        tx.send(AppEvent::BeatPulse {
            bpm: 128.0,
            intensity: 0.6,
        })
        .unwrap();
        run_pump(&mut world);
        let messages = world.resource_mut::<Messages<BeatPulse>>();
        let mut cursor = messages.get_cursor();
        let events: Vec<_> = cursor.read(&*messages).collect();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].bpm, 128.0);
        assert_eq!(events[0].intensity, 0.6);
    }

    #[test]
    fn ai_finished_emits_message() {
        let (mut world, tx) = build_world();
        tx.send(AppEvent::Ai(AiStreamUpdate::Finished)).unwrap();
        let mut schedule = Schedule::default();
        schedule.add_systems(
            (
                pump_legacy_events,
                |mut reader: MessageReader<AiStreamFinished>| {
                    assert_eq!(reader.read().count(), 1);
                },
            )
                .chain(),
        );
        schedule.run(&mut world);
    }
}
