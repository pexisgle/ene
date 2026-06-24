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

use crate::event::ai::{
    AiPermissionRequested, AiStreamFinished, AiTextDelta, AiUserInputRequested, EmoteToken,
};
use crate::event::lifecycle::TickGtk;
use crate::event::settings::OpenSettings;
use crate::events::{AiStreamUpdate, AppEvent};
use crate::resource::{event_channels::EventChannels, exit::ExitRequested};

/// Drains the legacy `AppEvent` bus and writes typed [`Message`]s.
/// Runs in the `First` stage so any `Update` systems see fresh
/// events.
///
/// Phase 7.5: this pump is the single producer of every typed
/// `Message` the desktop runtime consumes; the legacy
/// `PendingActions` mirror has been removed. Per-frame actions
/// are now handled by the consumer systems in
/// `system::ui_consumers.rs` reading the `Message` queue.
pub fn pump_legacy_events(
    mut channels: ResMut<EventChannels>,
    mut exit: ResMut<ExitRequested>,
    mut text_delta: MessageWriter<AiTextDelta>,
    mut permission: MessageWriter<AiPermissionRequested>,
    mut user_input: MessageWriter<AiUserInputRequested>,
    mut finished: MessageWriter<AiStreamFinished>,
    mut emote: MessageWriter<EmoteToken>,
    mut open_settings: MessageWriter<OpenSettings>,
    #[cfg(target_os = "linux")] mut tick_gtk: MessageWriter<TickGtk>,
) {
    while let Ok(event) = channels.rx.try_recv() {
        translate_event(
            event,
            &mut exit,
            &mut text_delta,
            &mut permission,
            &mut user_input,
            &mut finished,
            &mut emote,
            &mut open_settings,
        );
    }
    // Phase 7.5: publish a `TickGtk` every frame on Linux so the
    // tray icon library makes progress. The actual `tick_gtk()`
    // call still lives in `Runtime::about_to_wait` because the
    // `Rc<RefCell<TrayHandle>>` is not `Send + Sync`.
    #[cfg(target_os = "linux")]
    tick_gtk.write(TickGtk);
}

#[allow(clippy::too_many_arguments)]
fn translate_event(
    event: AppEvent,
    exit: &mut ExitRequested,
    text_delta: &mut MessageWriter<AiTextDelta>,
    permission: &mut MessageWriter<AiPermissionRequested>,
    user_input: &mut MessageWriter<AiUserInputRequested>,
    finished: &mut MessageWriter<AiStreamFinished>,
    emote: &mut MessageWriter<EmoteToken>,
    open_settings: &mut MessageWriter<OpenSettings>,
) {
    match event {
        AppEvent::Quit => {
            exit.0 = true;
        }
        AppEvent::Tray(crate::events::TrayAction::Quit) => {
            exit.0 = true;
        }
        AppEvent::Tray(crate::events::TrayAction::OpenSettings { page }) => {
            open_settings.write(OpenSettings { page });
        }
        AppEvent::Ai(AiStreamUpdate::TextDelta(text)) => {
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
        AppEvent::EmoteToken(name) => {
            emote.write(EmoteToken(name));
        }
        _ => {}
    }
}

/// Helper used by `pump_legacy_events` to indicate the GTK pump
/// should run this frame. Phase 7.5: replaced by the
/// `Messages<TickGtk>` queue; the consumer system
/// `tick_gtk_system` (Phase 7.4) handles the tick.
#[allow(
    dead_code,
    reason = "Replaced by Messages<TickGtk>; kept for API symmetry"
)]
pub fn mark_gtk_tick() {}

/// Suppress unused-imports for the `Instant` import in non-tray builds.
fn _force_link(_: Instant) {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::TrayAction;
    use crate::settings_ui::PageKind;
    use bevy_ecs::message::MessageReader;
    use ene_core::RequestId;
    use tokio::sync::mpsc;

    fn build_world() -> (World, mpsc::UnboundedSender<AppEvent>) {
        let (tx, rx) = mpsc::unbounded_channel::<AppEvent>();
        let mut world = World::new();
        world.insert_resource(EventChannels { tx: tx.clone(), rx });
        world.insert_resource(ExitRequested::default());
        world.init_resource::<Messages<AiTextDelta>>();
        world.init_resource::<Messages<AiStreamFinished>>();
        world.init_resource::<Messages<AiPermissionRequested>>();
        world.init_resource::<Messages<AiUserInputRequested>>();
        world.init_resource::<Messages<EmoteToken>>();
        world.init_resource::<Messages<OpenSettings>>();
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
            request_id: RequestId::new("req-1"),
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
        tx.send(AppEvent::EmoteToken("happy".to_string())).unwrap();
        run_pump(&mut world);
        let messages = world.resource_mut::<Messages<EmoteToken>>();
        let mut cursor = messages.get_cursor();
        let events: Vec<_> = cursor.read(&*messages).collect();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].0, "happy");
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
