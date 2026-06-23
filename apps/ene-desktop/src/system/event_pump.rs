//! Systems that drain the legacy `AppEvent` bus and translate it
//! into typed `bevy_ecs::message::Message`s + `PendingActions`.
//!
//! The pump runs in the `First` stage. Messages written by the pump
//! are normally consumed by `Update` stage systems; the
//! `PendingActions` resource is the bridge the legacy
//! `Runtime::about_to_wait` body reads to apply per-frame work
//! before handing control back to the winit event loop.
use std::time::Instant;

use bevy_ecs::prelude::*;

use crate::character_state::EmotionCommand;
use crate::event::ai::{
    AiPermissionRequested, AiStreamFinished, AiTextDelta, AiUserInputRequested, EmoteToken,
};
use crate::event::settings::OpenSettings;
use crate::events::{AiStreamUpdate, AppEvent};
use crate::resource::{
    event_channels::EventChannels, exit::ExitRequested, pending_actions::PendingActions,
};
use crate::settings::{PendingPermission, PendingUserInput};

/// Drains the legacy `AppEvent` bus, writes typed [`Message`]s, and
/// populates [`PendingActions`]. Runs in the `First` stage so any
/// `Update` systems see fresh events.
pub fn pump_legacy_events(
    mut channels: ResMut<EventChannels>,
    mut pending: ResMut<PendingActions>,
    mut exit: ResMut<ExitRequested>,
    mut text_delta: MessageWriter<AiTextDelta>,
    mut permission: MessageWriter<AiPermissionRequested>,
    mut user_input: MessageWriter<AiUserInputRequested>,
    mut finished: MessageWriter<AiStreamFinished>,
    mut emote: MessageWriter<EmoteToken>,
    mut open_settings: MessageWriter<OpenSettings>,
) {
    while let Ok(event) = channels.rx.try_recv() {
        translate_event(
            event,
            &mut pending,
            &mut exit,
            &mut text_delta,
            &mut permission,
            &mut user_input,
            &mut finished,
            &mut emote,
            &mut open_settings,
            None,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn translate_event(
    event: AppEvent,
    pending: &mut PendingActions,
    exit: &mut ExitRequested,
    text_delta: &mut MessageWriter<AiTextDelta>,
    permission: &mut MessageWriter<AiPermissionRequested>,
    user_input: &mut MessageWriter<AiUserInputRequested>,
    finished: &mut MessageWriter<AiStreamFinished>,
    emote: &mut MessageWriter<EmoteToken>,
    open_settings: &mut MessageWriter<OpenSettings>,
    now_secs: Option<f64>,
) {
    match event {
        AppEvent::Quit => {
            pending.quit = true;
            exit.0 = true;
        }
        AppEvent::Tray(crate::events::TrayAction::Quit) => {
            pending.quit = true;
            exit.0 = true;
        }
        AppEvent::Tray(crate::events::TrayAction::OpenSettings { page }) => {
            pending.open_settings = Some(page);
            open_settings.write(OpenSettings { page });
        }
        AppEvent::Ai(AiStreamUpdate::TextDelta(text)) => {
            pending.ai_text_deltas.push(text.clone());
            text_delta.write(AiTextDelta(text));
        }
        AppEvent::Ai(AiStreamUpdate::Finished | AiStreamUpdate::Error(_)) => {
            finished.write(AiStreamFinished);
        }
        AppEvent::Ai(AiStreamUpdate::PermissionRequired {
            request_id,
            action,
            target,
            description,
        }) => {
            pending.ai_permission = Some(PendingPermission {
                request_id: request_id.clone(),
                action: action.clone(),
                target: target.clone(),
                description: if description.is_empty() {
                    None
                } else {
                    Some(description.clone())
                },
            });
            pending.open_settings = Some(Some(crate::settings_ui::PageKind::Ai));
            permission.write(AiPermissionRequested {
                request_id,
                action,
                target,
                description,
            });
        }
        AppEvent::Ai(AiStreamUpdate::UserInputRequired { request_id, prompt }) => {
            pending.ai_user_input = Some(PendingUserInput {
                request_id: request_id.clone(),
                prompt: prompt.clone(),
            });
            pending.ai_user_input_drafts = prompt.items.len();
            pending.open_settings = Some(Some(crate::settings_ui::PageKind::Ai));
            user_input.write(AiUserInputRequested { request_id, prompt });
        }
        AppEvent::EmoteToken(name) => {
            let target_time = now_secs.unwrap_or(0.0);
            pending.emotion_commands.push_back(EmotionCommand {
                emotion: name.clone(),
                target_time,
                hold_secs: 4.0,
                weight: 1.0,
            });
            emote.write(EmoteToken(name));
        }
        _ => {}
    }
}

/// Helper used by `pump_legacy_events` to indicate the GTK pump
/// should run this frame. Mirrors the legacy behaviour.
#[expect(dead_code, reason = "Used by tray plugin in Phase 6")]
pub fn mark_gtk_tick(pending: &mut PendingActions) {
    pending.tick_gtk = true;
}

/// Suppress unused-imports for the `Instant` import in non-tray builds.
fn _force_link(_: Instant) {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::TrayAction;
    use crate::settings_ui::PageKind;
    use bevy_ecs::prelude::*;
    use ene_core::RequestId;
    use tokio::sync::mpsc;

    fn build_world() -> (World, mpsc::UnboundedSender<AppEvent>) {
        let (tx, rx) = mpsc::unbounded_channel::<AppEvent>();
        let mut world = World::new();
        world.insert_resource(EventChannels { tx: tx.clone(), rx });
        world.insert_resource(PendingActions::default());
        world.insert_resource(ExitRequested::default());
        world.init_resource::<Messages<AiTextDelta>>();
        world.init_resource::<Messages<AiStreamFinished>>();
        world.init_resource::<Messages<AiPermissionRequested>>();
        world.init_resource::<Messages<AiUserInputRequested>>();
        world.init_resource::<Messages<EmoteToken>>();
        world.init_resource::<Messages<OpenSettings>>();
        (world, tx)
    }

    fn run_pump(world: &mut World) {
        let mut schedule = Schedule::default();
        schedule.add_systems(pump_legacy_events);
        schedule.run(world);
    }

    #[test]
    fn quit_event_sets_exit_and_pending() {
        let (mut world, tx) = build_world();
        tx.send(AppEvent::Quit).unwrap();
        run_pump(&mut world);
        assert!(world.resource::<ExitRequested>().0);
        assert!(world.resource::<PendingActions>().quit);
    }

    #[test]
    fn tray_quit_sets_exit() {
        let (mut world, tx) = build_world();
        tx.send(AppEvent::Tray(TrayAction::Quit)).unwrap();
        run_pump(&mut world);
        assert!(world.resource::<ExitRequested>().0);
        assert!(world.resource::<PendingActions>().quit);
    }

    #[test]
    fn open_settings_sets_page() {
        let (mut world, tx) = build_world();
        tx.send(AppEvent::Tray(TrayAction::OpenSettings {
            page: Some(PageKind::Ai),
        }))
        .unwrap();
        run_pump(&mut world);
        let pending = world.resource::<PendingActions>();
        assert_eq!(pending.open_settings, Some(Some(PageKind::Ai)));
    }

    #[test]
    fn permission_required_sets_pending() {
        let (mut world, tx) = build_world();
        tx.send(AppEvent::Ai(AiStreamUpdate::PermissionRequired {
            request_id: RequestId::new("req-1"),
            action: "read".to_string(),
            target: "file.txt".to_string(),
            description: "needs approval".to_string(),
        }))
        .unwrap();
        run_pump(&mut world);
        let pending = world.resource::<PendingActions>();
        let perm = pending.ai_permission.as_ref().expect("permission");
        assert_eq!(perm.action, "read");
        assert_eq!(perm.target, "file.txt");
        assert_eq!(perm.description.as_deref(), Some("needs approval"));
        assert_eq!(pending.open_settings, Some(Some(PageKind::Ai)));
    }

    #[test]
    fn text_delta_appends_to_buffer() {
        let (mut world, tx) = build_world();
        tx.send(AppEvent::Ai(AiStreamUpdate::TextDelta(
            "hello ".to_string(),
        )))
        .unwrap();
        tx.send(AppEvent::Ai(AiStreamUpdate::TextDelta("world".to_string())))
            .unwrap();
        run_pump(&mut world);
        let pending = world.resource::<PendingActions>();
        assert_eq!(pending.ai_text_deltas, vec!["hello ", "world"]);
    }

    #[test]
    fn emote_token_enqueues_command() {
        let (mut world, tx) = build_world();
        tx.send(AppEvent::EmoteToken("happy".to_string())).unwrap();
        run_pump(&mut world);
        let pending = world.resource::<PendingActions>();
        assert_eq!(pending.emotion_commands.len(), 1);
        assert_eq!(pending.emotion_commands[0].emotion, "happy");
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
