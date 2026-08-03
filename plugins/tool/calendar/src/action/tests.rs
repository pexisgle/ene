//! End-to-end action tests through the approval gate: gate ordering
//! (permission prompt before any store mutation), retry request-id matching
//! after approval, and the preview content delivered to the user.

use super::*;
use crate::approval::actions;
use crate::state::CalendarState;
use crate::store::CalendarKind;
use crate::test_db::spawn_mock_db;
use ene_plugin::ToolAction;
use ene_plugin_proto::ToolError;
use ene_plugin_proto::transport::cleanup_path;
use std::path::PathBuf;
use std::sync::Arc;

struct TestEnv {
    state: CalendarState,
    socket_path: PathBuf,
    handle: tokio::task::JoinHandle<()>,
}

async fn make_env() -> TestEnv {
    let (socket_path, handle) = spawn_mock_db().await;
    let state = CalendarState::new();
    state.set_db_socket(socket_path.to_string_lossy().to_string());
    state.set_db_auth_token(Some("test-token".to_string()));
    TestEnv {
        state,
        socket_path,
        handle,
    }
}

impl Drop for TestEnv {
    fn drop(&mut self) {
        self.handle.abort();
        cleanup_path(&self.socket_path);
    }
}

async fn seed_writable_account(state: &CalendarState) {
    let store = state.ensure_store().await.expect("connect to mock db");
    store
        .add_account("a1", "Work", CalendarKind::Local)
        .await
        .expect("seed account");
    store
        .set_permissions("a1", None, Some(true))
        .await
        .expect("grant write");
}

fn create_args(title: &str) -> String {
    serde_json::json!({
        "calendar_id": "a1",
        "title": title,
        "start": "2026-08-03T10:00:00+09:00",
        "end": "2026-08-03T10:30:00+09:00",
    })
    .to_string()
}

fn expect_permission_required(result: Result<String, ToolError>) -> String {
    match result {
        Err(ToolError::PermissionRequired { request_id, .. }) => request_id,
        other => panic!("expected PermissionRequired, got {other:?}"),
    }
}

#[tokio::test]
async fn create_event_gates_before_any_store_mutation() {
    let env = make_env().await;
    seed_writable_account(&env.state).await;

    let action = CreateEventAction::new(Arc::new(env.state.clone()));
    let args = create_args("Standup");
    expect_permission_required(action.execute(&args).await);

    let store = env.state.ensure_store().await.expect("store");
    assert!(
        store
            .list_events("a1", None, None, false)
            .await
            .expect("list events")
            .is_empty(),
        "denied create must not touch the store"
    );
}

#[tokio::test]
async fn create_event_retry_matches_the_approved_request_id() {
    let env = make_env().await;
    seed_writable_account(&env.state).await;

    let action = CreateEventAction::new(Arc::new(env.state.clone()));
    let args = create_args("Standup");
    let request_id = expect_permission_required(action.execute(&args).await);
    let description = match action.execute(&args).await {
        Err(ToolError::PermissionRequired { description, .. }) => description,
        other => panic!("expected PermissionRequired, got {other:?}"),
    };
    assert!(
        description.contains("Create event 'Standup'"),
        "preview names the event content: {description}"
    );

    env.state.gate().approve_request(&request_id);
    let result = action
        .execute(&args)
        .await
        .expect("approved create must succeed");
    assert!(
        result.contains("Standup"),
        "result returns the created event"
    );
}

#[tokio::test]
async fn create_event_approval_does_not_survive_the_turn() {
    let env = make_env().await;
    seed_writable_account(&env.state).await;

    let action = CreateEventAction::new(Arc::new(env.state.clone()));
    let args = create_args("Standup");
    let request_id = expect_permission_required(action.execute(&args).await);
    env.state.gate().approve_request(&request_id);
    action
        .execute(&args)
        .await
        .expect("approved within the turn");

    env.state.gate().on_call_context("conv-1", Some("turn-2"));
    expect_permission_required(action.execute(&args).await);
}

#[tokio::test]
async fn update_event_previews_timezone_only_changes() {
    let env = make_env().await;
    seed_writable_account(&env.state).await;
    let store = env.state.ensure_store().await.expect("store");
    let event = store
        .create_event(
            "a1",
            &serde_json::from_value(serde_json::json!({
                "title": "Standup",
                "start": "2026-08-03T10:00:00+09:00",
                "end": "2026-08-03T10:30:00+09:00",
                "timezone": "Asia/Tokyo",
            }))
            .expect("input"),
        )
        .await
        .expect("seed event");

    let action = UpdateEventAction::new(Arc::new(env.state.clone()));
    let args = serde_json::json!({
        "calendar_id": "a1",
        "event_id": event.id,
        "timezone": "America/New_York",
    })
    .to_string();
    let request_id = expect_permission_required(action.execute(&args).await);
    let description = match action.execute(&args).await {
        Err(ToolError::PermissionRequired { description, .. }) => description,
        other => panic!("expected PermissionRequired, got {other:?}"),
    };
    assert!(
        description.contains("timezone: Asia/Tokyo -> America/New_York"),
        "timezone-only update must be previewed: {description}"
    );
    assert!(!description.contains("no changes"));

    env.state.gate().approve_request(&request_id);
    let result = action
        .execute(&args)
        .await
        .expect("approved update must succeed");
    assert!(result.contains("America/New_York"));
}

#[tokio::test]
async fn cancel_event_marks_the_event_cancelled() {
    let env = make_env().await;
    seed_writable_account(&env.state).await;
    let store = env.state.ensure_store().await.expect("store");
    let event = store
        .create_event(
            "a1",
            &serde_json::from_value(serde_json::json!({
                "title": "Standup",
                "start": "2026-08-03T10:00:00+09:00",
                "end": "2026-08-03T10:30:00+09:00",
            }))
            .expect("input"),
        )
        .await
        .expect("seed event");

    let action = CancelEventAction::new(Arc::new(env.state.clone()));
    let args = serde_json::json!({
        "calendar_id": "a1",
        "event_id": event.id,
    })
    .to_string();
    let request_id = expect_permission_required(action.execute(&args).await);
    env.state.gate().approve_request(&request_id);
    let result = action
        .execute(&args)
        .await
        .expect("approved cancel must succeed");
    assert!(result.contains("cancelled"));

    let cancelled = store
        .get_event("a1", &event.id)
        .await
        .expect("soft-cancelled event stays stored");
    assert_eq!(cancelled.status, "cancelled");
}

#[tokio::test]
async fn gate_action_names_are_stable_contracts() {
    assert_eq!(actions::CALENDAR_ADD, "CalendarAdd");
    assert_eq!(actions::CALENDAR_WRITE, "CalendarWrite");
    assert_eq!(actions::CALENDAR_DELETE, "CalendarDelete");
    assert_eq!(actions::CALENDAR_PERMISSION, "CalendarPermission");
}
