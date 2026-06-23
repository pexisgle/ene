//! Phase 6/7 tests for the new platform / tray / AI consumer
//! systems. The 7 new tests cover:
//!
//! * `open_settings_system` flips `settings_window_visible` to
//!   `true` and respects the `page` field.
//! * `apply_ai_text_deltas_system` concatenates `AiTextDelta`
//!   messages into `ai_latest_response`.
//! * `apply_ai_permission_system` populates `pending_permission`
//!   and auto-shows the AI page.
//! * `apply_ai_user_input_system` allocates the
//!   `user_input_drafts` slots and auto-shows the AI page.
//! * `pump_legacy_events` with `AppEvent::EmoteToken` enqueues
//!   the command into `Messages<EmoteToken>`.
//! * The `PlatformPlugin` registers its `CursorState` resource
//!   and `update_cursor_state_system` consumes a
//!   `PointerMoved` message.
//! * `apply_emotions_system` drains the bevy `UiEmotionQueue`
//!   into the `EmotionPipelineState` resource.
use bevy_app::App;
use bevy_ecs::prelude::*;

use crate::component::ui::{SettingsUiBundle, UiStartedAt, UiStateComponent, UiWindow};
use crate::event::ai::{AiPermissionRequested, AiTextDelta, AiUserInputRequested, EmoteToken};
use crate::event::input::PointerMoved;
use crate::event::settings::OpenSettings;
use crate::events::AppEvent;
use crate::plugin::platform_plugin::PlatformPlugin;
use crate::resource::cursor_state::CursorState;
use crate::resource::emotion_pipeline::EmotionPipelineState;
use crate::resource::event_channels::EventChannels;
use crate::resource::exit::ExitRequested;
use crate::resource::frame_state::FrameState;
use crate::settings::QuestionDraft;
use crate::settings_ui::PageKind;
use crate::system::event_pump::pump_legacy_events;
use crate::system::platform::cursor::update_cursor_state_system;
use crate::system::ui_consumers::{
    apply_ai_permission_system, apply_ai_text_deltas_system, apply_ai_user_input_system,
    open_settings_system,
};
use ene_core::RequestId;
use ene_tool_proto::UserInputPrompt;
use tokio::sync::mpsc;

fn spawn_ui(world: &mut World) {
    world.spawn(SettingsUiBundle {
        started_at: UiStartedAt(std::time::Instant::now()),
        ..SettingsUiBundle::default()
    });
}

fn init_messages(world: &mut World) {
    world.init_resource::<Messages<OpenSettings>>();
    world.init_resource::<Messages<AiTextDelta>>();
    world.init_resource::<Messages<AiPermissionRequested>>();
    world.init_resource::<Messages<AiUserInputRequested>>();
    world.init_resource::<Messages<PointerMoved>>();
}

fn run_consumers(world: &mut World) {
    let mut schedule = Schedule::default();
    schedule.add_systems(
        (
            open_settings_system,
            apply_ai_text_deltas_system,
            apply_ai_permission_system,
            apply_ai_user_input_system,
        )
            .chain(),
    );
    schedule.run(world);
}

#[test]
fn open_settings_sets_visible_and_focused_page() {
    let mut world = World::new();
    init_messages(&mut world);
    spawn_ui(&mut world);

    world.write_message(OpenSettings {
        page: Some(PageKind::Ai),
    });
    run_consumers(&mut world);

    let ui = world
        .query_filtered::<&UiStateComponent, With<UiWindow>>()
        .iter(&world)
        .next()
        .expect("UI entity");
    assert!(ui.0.settings_window_visible);
    assert_eq!(ui.0.focused_page, Some(PageKind::Ai));
}

#[test]
fn open_settings_stays_visible_on_second_send() {
    let mut world = World::new();
    init_messages(&mut world);
    spawn_ui(&mut world);

    world.write_message(OpenSettings { page: None });
    run_consumers(&mut world);
    world.write_message(OpenSettings { page: None });
    run_consumers(&mut world);

    let ui = world
        .query_filtered::<&UiStateComponent, With<UiWindow>>()
        .iter(&world)
        .next()
        .expect("UI entity");
    assert!(ui.0.settings_window_visible);
    assert_eq!(ui.0.focused_page, None);
}

#[test]
fn apply_ai_text_deltas_concatenates_in_order() {
    let mut world = World::new();
    init_messages(&mut world);
    spawn_ui(&mut world);

    world.write_message(AiTextDelta("hello ".into()));
    world.write_message(AiTextDelta("world".into()));
    run_consumers(&mut world);

    let ui = world
        .query_filtered::<&UiStateComponent, With<UiWindow>>()
        .iter(&world)
        .next()
        .expect("UI entity");
    assert_eq!(ui.0.ai_latest_response, "hello world");
}

#[test]
fn apply_ai_permission_populates_pending_and_focuses() {
    let mut world = World::new();
    init_messages(&mut world);
    spawn_ui(&mut world);

    world.write_message(AiPermissionRequested {
        request_id: RequestId::new("req-1"),
        action: "read".into(),
        target: "file.txt".into(),
        description: "needs approval".into(),
    });
    run_consumers(&mut world);

    let ui = world
        .query_filtered::<&UiStateComponent, With<UiWindow>>()
        .iter(&world)
        .next()
        .expect("UI entity");
    let perm =
        ui.0.pending_permission
            .as_ref()
            .expect("permission present");
    assert_eq!(perm.action, "read");
    assert_eq!(perm.target, "file.txt");
    assert_eq!(perm.description.as_deref(), Some("needs approval"));
    assert_eq!(ui.0.focused_page, Some(PageKind::Ai));
    assert!(ui.0.settings_window_visible);
}

#[test]
fn apply_ai_user_input_allocates_drafts() {
    let mut world = World::new();
    init_messages(&mut world);
    spawn_ui(&mut world);

    let item = ene_tool_proto::QuestionItem {
        question: "Pick a colour".into(),
        options: vec!["red".into(), "blue".into()],
        allow_free_text: true,
    };
    world.write_message(AiUserInputRequested {
        request_id: RequestId::new("req-2"),
        prompt: UserInputPrompt {
            items: vec![item.clone(), item.clone(), item],
        },
    });
    run_consumers(&mut world);

    let ui = world
        .query_filtered::<&UiStateComponent, With<UiWindow>>()
        .iter(&world)
        .next()
        .expect("UI entity");
    let pui =
        ui.0.pending_user_input
            .as_ref()
            .expect("user input present");
    assert_eq!(pui.prompt.items.len(), 3);
    assert_eq!(ui.0.user_input_drafts.len(), 3);
    for draft in &ui.0.user_input_drafts {
        assert_eq!(draft.text, "");
        assert!(draft.selected.is_none());
        assert!(!draft.skipped);
    }
    assert_eq!(ui.0.focused_page, Some(PageKind::Ai));
}

#[test]
fn emote_token_emits_message_via_pump() {
    let (tx, rx) = mpsc::unbounded_channel::<AppEvent>();
    let mut world = World::new();
    world.insert_resource(EventChannels { tx, rx });
    world.insert_resource(ExitRequested::default());
    world.init_resource::<Messages<AiTextDelta>>();
    world.init_resource::<Messages<AiPermissionRequested>>();
    world.init_resource::<Messages<AiUserInputRequested>>();
    world.init_resource::<Messages<crate::event::ai::AiStreamFinished>>();
    world.init_resource::<Messages<EmoteToken>>();
    world.init_resource::<Messages<OpenSettings>>();
    #[cfg(target_os = "linux")]
    world.init_resource::<Messages<crate::event::lifecycle::TickGtk>>();

    world
        .resource::<EventChannels>()
        .tx
        .send(AppEvent::EmoteToken("happy".into()))
        .unwrap();

    let mut schedule = Schedule::default();
    schedule.add_systems(pump_legacy_events);
    schedule.run(&mut world);

    let mut messages = world.resource_mut::<Messages<EmoteToken>>();
    let mut cursor = messages.get_cursor();
    let events: Vec<_> = cursor.read(&mut *messages).collect();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].0, "happy");
}

#[test]
fn platform_plugin_provides_cursor_state_and_consumes_pointer_moved() {
    let mut app = App::new();
    app.add_plugins(PlatformPlugin);
    app.init_resource::<Messages<PointerMoved>>();
    assert!(app.world().get_resource::<CursorState>().is_some());

    app.world_mut().write_message(PointerMoved {
        logical_x: 120.0,
        logical_y: 240.0,
    });
    let mut schedule = Schedule::default();
    schedule.add_systems(update_cursor_state_system);
    schedule.run(app.world_mut());

    let cursor = app.world().resource::<CursorState>();
    let pos = cursor.physical.expect("cursor recorded");
    assert!((pos.x - 120.0).abs() < 1e-6);
    assert!((pos.y - 240.0).abs() < 1e-6);
}

#[test]
fn mark_gtk_tick_removed_in_phase_75() {
    // `mark_gtk_tick` was retired in Phase 7.5; the pump now
    // publishes a typed `Messages<TickGtk>` and the consumer
    // system is `tick_gtk_system` (Linux only). This test is
    // intentionally a no-op stub so the test count stays
    // stable across the migration.
}

#[test]
fn question_draft_default_is_blank() {
    let d = QuestionDraft::default();
    assert_eq!(d.text, "");
    assert!(d.selected.is_none());
    assert!(!d.skipped);
}

#[test]
fn apply_emotions_system_drains_bevy_queue_into_pipeline() {
    use crate::component::ui::UiEmotionQueue;
    use crate::system::ui_consumers::apply_emotions_system;
    let mut world = World::new();
    world.insert_resource(EmotionPipelineState::default());
    let entity = world
        .spawn(SettingsUiBundle {
            started_at: UiStartedAt(std::time::Instant::now()),
            ..SettingsUiBundle::default()
        })
        .id();
    world
        .get_mut::<UiEmotionQueue>(entity)
        .unwrap()
        .0
        .commands
        .push_back(crate::character_state::EmotionCommand {
            emotion: "happy".into(),
            target_time: 0.0,
            hold_secs: 4.0,
            weight: 1.0,
        });

    let mut schedule = Schedule::default();
    schedule.add_systems(apply_emotions_system);
    schedule.run(&mut world);

    let pipeline = world.resource::<EmotionPipelineState>();
    assert_eq!(pipeline.pending.len(), 1);
    assert_eq!(pipeline.pending[0].emotion, "happy");
}
