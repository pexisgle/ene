//! Integration tests for [`MemoryStore`].

use super::memory::strip_tags_footer;
use chrono::{DateTime, Utc};

use super::*;

async fn setup_store() -> MemoryStore {
    MemoryStore::open_in_memory(4).await.unwrap()
}

use super::schedule::FireClaimMode;
use chrono::TimeZone;
use ene_core::{
    NewSchedule, ScheduleAction, ScheduleConfirmation, ScheduleKind, ScheduleRunStatus,
};

fn new_cron_schedule(name: &str, cron_expr: &str, timezone: &str) -> NewSchedule {
    NewSchedule {
        name: name.to_string(),
        kind: ScheduleKind::Cron,
        timezone: timezone.to_string(),
        cron_expr: Some(cron_expr.to_string()),
        interval_secs: None,
        start_at: None,
        action: ScheduleAction::Prompt {
            text: "reminder".to_string(),
            allow_tools: false,
        },
        confirmation: ScheduleConfirmation::None,
        max_retries: 0,
        retry_delay_secs: 60,
    }
}

fn at(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(y, mo, d, h, mi, 0).unwrap()
}

#[tokio::test]
async fn schedule_crud_roundtrip_and_validation() {
    let store = setup_store().await;
    let now = at(2026, 8, 4, 12, 0);

    let inserted = store
        .insert_schedule(&new_cron_schedule("daily", "0 9 * * *", "Asia/Tokyo"), now)
        .await
        .unwrap();
    // 09:00 JST = 00:00 UTC; strictly after 12:00 UTC on the 4th, so the
    // first occurrence is 2026-08-05T00:00:00Z.
    assert_eq!(inserted.next_run_at, Some(at(2026, 8, 5, 0, 0)));
    assert_eq!(
        store.get_schedule(inserted.id).await.unwrap().unwrap().name,
        "daily"
    );
    assert_eq!(
        store
            .get_schedule_by_name("daily")
            .await
            .unwrap()
            .unwrap()
            .id,
        inserted.id
    );
    assert_eq!(store.list_schedules().await.unwrap().len(), 1);

    let invalid = new_cron_schedule("broken", "not a cron", "UTC");
    assert!(matches!(
        store.insert_schedule(&invalid, now).await,
        Err(EneMemoryError::InvalidSchedule(..))
    ));

    assert!(
        store
            .set_schedule_enabled(inserted.id, false, now)
            .await
            .unwrap()
    );
    let paused = store.get_schedule(inserted.id).await.unwrap().unwrap();
    assert!(!paused.enabled);

    assert!(store.delete_schedule(inserted.id).await.unwrap());
    assert!(!store.delete_schedule(inserted.id).await.unwrap());
    assert!(store.get_schedule(inserted.id).await.unwrap().is_none());
}

#[tokio::test]
async fn claim_fire_advances_and_records_run() {
    let store = setup_store().await;
    let now = at(2026, 8, 4, 12, 0);
    let schedule = store
        .insert_schedule(&new_cron_schedule("daily", "0 9 * * *", "UTC"), now)
        .await
        .unwrap();
    let first_fire = schedule.next_run_at.unwrap();

    let claimed = store
        .claim_fire(schedule.id, first_fire, now, FireClaimMode::Execute)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(claimed.run_id, 1);
    assert!(!claimed.is_retry);

    let after = store.get_schedule(schedule.id).await.unwrap().unwrap();
    assert_eq!(
        after.next_run_at,
        Some(first_fire + chrono::Duration::days(1))
    );
    assert_eq!(after.run_count, 1);
    assert_eq!(after.last_status, Some(ScheduleRunStatus::Running));
    let runs = store.list_runs(schedule.id, 10).await.unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].status, ScheduleRunStatus::Running);
    assert_eq!(runs[0].scheduled_at, first_fire);

    store
        .finish_run(
            schedule.id,
            claimed.run_id,
            ScheduleRunStatus::Success,
            None,
            now,
        )
        .await
        .unwrap();
    let runs = store.list_runs(schedule.id, 10).await.unwrap();
    assert_eq!(runs[0].status, ScheduleRunStatus::Success);
    assert!(runs[0].finished_at.is_some());
    assert_eq!(
        store
            .get_schedule(schedule.id)
            .await
            .unwrap()
            .unwrap()
            .last_status,
        Some(ScheduleRunStatus::Success)
    );
}

#[tokio::test]
async fn claim_fire_is_idempotent_for_stale_disabled_or_unknown() {
    let store = setup_store().await;
    let now = at(2026, 8, 4, 12, 0);
    let schedule = store
        .insert_schedule(&new_cron_schedule("daily", "0 9 * * *", "UTC"), now)
        .await
        .unwrap();
    let fire = schedule.next_run_at.unwrap();

    // Wrong scheduled_at: ignored.
    assert!(
        store
            .claim_fire(
                schedule.id,
                fire + chrono::Duration::hours(1),
                now,
                FireClaimMode::Execute
            )
            .await
            .unwrap()
            .is_none()
    );
    // Unknown schedule: ignored.
    assert!(
        store
            .claim_fire(999_999, fire, now, FireClaimMode::Execute)
            .await
            .unwrap()
            .is_none()
    );
    // Claiming twice with the same scheduled_at: the second is stale.
    store
        .claim_fire(schedule.id, fire, now, FireClaimMode::Execute)
        .await
        .unwrap()
        .unwrap();
    assert!(
        store
            .claim_fire(schedule.id, fire, now, FireClaimMode::Execute)
            .await
            .unwrap()
            .is_none()
    );
    // Disabled: ignored.
    store
        .set_schedule_enabled(schedule.id, false, now)
        .await
        .unwrap();
    let next = store
        .get_schedule(schedule.id)
        .await
        .unwrap()
        .unwrap()
        .next_run_at
        .unwrap();
    assert!(
        store
            .claim_fire(schedule.id, next, now, FireClaimMode::Execute)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(store.list_runs(schedule.id, 10).await.unwrap().len(), 1);
}

#[tokio::test]
async fn skipped_fires_are_recorded_and_advance() {
    let store = setup_store().await;
    let now = at(2026, 8, 4, 12, 0);
    let schedule = store
        .insert_schedule(&new_cron_schedule("hourly", "0 * * * *", "UTC"), now)
        .await
        .unwrap();
    let fire = schedule.next_run_at.unwrap();
    assert_eq!(fire, at(2026, 8, 4, 13, 0));

    store
        .claim_fire(schedule.id, fire, fire, FireClaimMode::SkipBusy)
        .await
        .unwrap()
        .unwrap();
    let after = store.get_schedule(schedule.id).await.unwrap().unwrap();
    assert_eq!(after.last_status, Some(ScheduleRunStatus::SkippedBusy));
    // Busy-skip preserves cadence: the next occurrence is one hour later.
    assert_eq!(after.next_run_at, Some(fire + chrono::Duration::hours(1)));

    let next_fire = after.next_run_at.unwrap();
    let late = next_fire + chrono::Duration::hours(5);
    store
        .claim_fire(schedule.id, next_fire, late, FireClaimMode::SkipLate)
        .await
        .unwrap()
        .unwrap();
    let after = store.get_schedule(schedule.id).await.unwrap().unwrap();
    // Late-skip jumps to the first occurrence after `now` (19:00), not the
    // missed 14:00 occurrence.
    assert_eq!(after.next_run_at, Some(at(2026, 8, 4, 20, 0)));
    assert_eq!(after.last_status, Some(ScheduleRunStatus::SkippedLate));
    let runs = store.list_runs(schedule.id, 10).await.unwrap();
    assert_eq!(runs.len(), 2);
    assert_eq!(runs[0].status, ScheduleRunStatus::SkippedLate);
    assert_eq!(runs[0].scheduled_at, next_fire);
}

#[tokio::test]
async fn failed_run_arms_retry_and_retry_claim_consumes_it() {
    let store = setup_store().await;
    let anchor = at(2026, 8, 4, 0, 0);
    let mut schedule = new_cron_schedule("retrying", "0 9 * * *", "UTC");
    schedule.kind = ScheduleKind::Interval;
    schedule.start_at = Some(anchor);
    schedule.interval_secs = Some(3600);
    schedule.cron_expr = None;
    schedule.max_retries = 2;
    schedule.retry_delay_secs = 30;
    let schedule = store.insert_schedule(&schedule, anchor).await.unwrap();
    let fire = schedule.next_run_at.unwrap();
    assert_eq!(fire, anchor);

    let claimed = store
        .claim_fire(schedule.id, fire, fire, FireClaimMode::Execute)
        .await
        .unwrap()
        .unwrap();
    store
        .finish_run(
            schedule.id,
            claimed.run_id,
            ScheduleRunStatus::Failed,
            Some("boom".to_string()),
            fire,
        )
        .await
        .unwrap();
    let after = store.get_schedule(schedule.id).await.unwrap().unwrap();
    let retry_at = fire + chrono::Duration::seconds(30);
    assert_eq!(after.next_run_at, Some(retry_at));
    assert_eq!(after.pending_retry_of_run_id, Some(claimed.run_id));
    assert_eq!(after.fail_count, 1);
    assert_eq!(after.last_status, Some(ScheduleRunStatus::Failed));

    // A skipped retry is re-armed instead of consumed: the pointer stays on
    // the original failed run and the next attempt is pushed by one delay.
    let skipped = store
        .claim_fire(schedule.id, retry_at, retry_at, FireClaimMode::SkipBusy)
        .await
        .unwrap()
        .unwrap();
    assert!(skipped.is_retry);
    let after = store.get_schedule(schedule.id).await.unwrap().unwrap();
    let second_retry_at = retry_at + chrono::Duration::seconds(30);
    assert_eq!(after.next_run_at, Some(second_retry_at));
    assert_eq!(after.pending_retry_of_run_id, Some(claimed.run_id));
    let runs = store.list_runs(schedule.id, 10).await.unwrap();
    assert_eq!(runs.len(), 2);
    assert_eq!(runs[0].status, ScheduleRunStatus::SkippedBusy);

    // Exhausting retries after a failed final attempt stops the chain.
    let second_attempt = store
        .claim_fire(
            schedule.id,
            second_retry_at,
            second_retry_at,
            FireClaimMode::Execute,
        )
        .await
        .unwrap()
        .unwrap();
    let runs = store.list_runs(schedule.id, 10).await.unwrap();
    assert_eq!(runs[0].retries, 1);
    assert_eq!(runs[0].retry_of_run_id, Some(claimed.run_id));
    store
        .finish_run(
            schedule.id,
            second_attempt.run_id,
            ScheduleRunStatus::Failed,
            Some("still broken".to_string()),
            second_retry_at,
        )
        .await
        .unwrap();
    let after = store.get_schedule(schedule.id).await.unwrap().unwrap();
    let third_retry_at = second_retry_at + chrono::Duration::seconds(30);
    assert_eq!(after.next_run_at, Some(third_retry_at));
    assert_eq!(after.pending_retry_of_run_id, Some(second_attempt.run_id));
    assert_eq!(after.fail_count, 2);

    // The third attempt is retries=2 == max_retries; its failure ends the
    // chain and the regular cadence resumes at 01:00.
    let third_attempt = store
        .claim_fire(
            schedule.id,
            third_retry_at,
            third_retry_at,
            FireClaimMode::Execute,
        )
        .await
        .unwrap()
        .unwrap();
    store
        .finish_run(
            schedule.id,
            third_attempt.run_id,
            ScheduleRunStatus::Failed,
            Some("still broken".to_string()),
            third_retry_at,
        )
        .await
        .unwrap();
    let after = store.get_schedule(schedule.id).await.unwrap().unwrap();
    assert!(after.pending_retry_of_run_id.is_none());
    assert_eq!(after.fail_count, 3);
    assert_eq!(after.next_run_at, Some(at(2026, 8, 4, 1, 0)));
    let runs = store.list_runs(schedule.id, 10).await.unwrap();
    assert_eq!(runs[0].retries, 2);
    assert_eq!(runs[0].retry_of_run_id, Some(second_attempt.run_id));
}

#[tokio::test]
async fn finish_run_is_idempotent_and_cascade_delete_removes_history() {
    let store = setup_store().await;
    let now = at(2026, 8, 4, 12, 0);
    let schedule = store
        .insert_schedule(&new_cron_schedule("daily", "0 9 * * *", "UTC"), now)
        .await
        .unwrap();
    let fire = schedule.next_run_at.unwrap();
    let claimed = store
        .claim_fire(schedule.id, fire, now, FireClaimMode::Execute)
        .await
        .unwrap()
        .unwrap();
    store
        .finish_run(
            schedule.id,
            claimed.run_id,
            ScheduleRunStatus::Success,
            None,
            now,
        )
        .await
        .unwrap();
    // A second finish (e.g. a stale retry outcome) is a no-op.
    store
        .finish_run(
            schedule.id,
            claimed.run_id,
            ScheduleRunStatus::Failed,
            Some("late".into()),
            now,
        )
        .await
        .unwrap();
    let runs = store.list_runs(schedule.id, 10).await.unwrap();
    assert_eq!(runs[0].status, ScheduleRunStatus::Success);
    assert_eq!(
        store
            .get_schedule(schedule.id)
            .await
            .unwrap()
            .unwrap()
            .fail_count,
        0
    );

    store.delete_schedule(schedule.id).await.unwrap();
    assert!(store.list_runs(schedule.id, 10).await.unwrap().is_empty());
}

#[tokio::test]
async fn reconcile_startup_marks_inflight_runs() {
    let store = setup_store().await;
    let now = at(2026, 8, 4, 12, 0);
    let schedule = store
        .insert_schedule(&new_cron_schedule("daily", "0 9 * * *", "UTC"), now)
        .await
        .unwrap();
    let fire = schedule.next_run_at.unwrap();
    store
        .claim_fire(schedule.id, fire, now, FireClaimMode::Execute)
        .await
        .unwrap();
    let second = store
        .get_schedule(schedule.id)
        .await
        .unwrap()
        .unwrap()
        .next_run_at
        .unwrap();
    store
        .claim_fire(schedule.id, second, now, FireClaimMode::AwaitConfirmation)
        .await
        .unwrap();

    store
        .reconcile_startup(now + chrono::Duration::minutes(1))
        .await
        .unwrap();
    let runs = store.list_runs(schedule.id, 10).await.unwrap();
    assert_eq!(runs[0].status, ScheduleRunStatus::TimedOut);
    assert_eq!(runs[1].status, ScheduleRunStatus::Interrupted);
    let schedule = store.get_schedule(schedule.id).await.unwrap().unwrap();
    assert_eq!(schedule.last_status, Some(ScheduleRunStatus::Interrupted));
}

#[tokio::test]
async fn reconcile_startup_keeps_awaiting_only_schedule_as_timed_out() {
    let store = setup_store().await;
    let now = at(2026, 8, 4, 12, 0);
    let schedule = store
        .insert_schedule(&new_cron_schedule("gated", "0 9 * * *", "UTC"), now)
        .await
        .unwrap();
    let fire = schedule.next_run_at.unwrap();
    store
        .claim_fire(schedule.id, fire, now, FireClaimMode::AwaitConfirmation)
        .await
        .unwrap();

    store
        .reconcile_startup(now + chrono::Duration::minutes(1))
        .await
        .unwrap();
    let runs = store.list_runs(schedule.id, 10).await.unwrap();
    assert_eq!(runs[0].status, ScheduleRunStatus::TimedOut);
    let schedule = store.get_schedule(schedule.id).await.unwrap().unwrap();
    assert_eq!(schedule.last_status, Some(ScheduleRunStatus::TimedOut));
}

#[tokio::test]
async fn approved_run_transitions_to_running_and_rejects_stale_timeout() {
    let store = setup_store().await;
    let now = at(2026, 8, 4, 12, 0);
    let mut schedule = new_cron_schedule("gated", "0 9 * * *", "UTC");
    schedule.confirmation = ScheduleConfirmation::Confirm;
    let schedule = store.insert_schedule(&schedule, now).await.unwrap();
    let fire = schedule.next_run_at.unwrap();
    let claimed = store
        .claim_fire(schedule.id, fire, now, FireClaimMode::AwaitConfirmation)
        .await
        .unwrap()
        .unwrap();

    // Approval starts execution: the row leaves `awaiting_approval`.
    store
        .mark_run_running(schedule.id, claimed.run_id, now)
        .await
        .unwrap();
    let runs = store.list_runs(schedule.id, 10).await.unwrap();
    assert_eq!(runs[0].status, ScheduleRunStatus::Running);

    // A stale confirmation timeout arriving mid-execution must be a no-op.
    store
        .finish_run(
            schedule.id,
            claimed.run_id,
            ScheduleRunStatus::TimedOut,
            Some("confirmation timed out".to_string()),
            now,
        )
        .await
        .unwrap();
    let runs = store.list_runs(schedule.id, 10).await.unwrap();
    assert_eq!(runs[0].status, ScheduleRunStatus::Running);

    // The real outcome still lands, and a timeout can never clobber it.
    store
        .finish_run(
            schedule.id,
            claimed.run_id,
            ScheduleRunStatus::Success,
            None,
            now,
        )
        .await
        .unwrap();
    let runs = store.list_runs(schedule.id, 10).await.unwrap();
    assert_eq!(runs[0].status, ScheduleRunStatus::Success);
}

#[tokio::test]
async fn next_due_time_excluding_skips_queued_schedules() {
    let store = setup_store().await;
    let now = at(2026, 8, 4, 12, 0);
    let a = store
        .insert_schedule(&new_cron_schedule("a", "30 12 * * *", "UTC"), now)
        .await
        .unwrap();
    let b = store
        .insert_schedule(&new_cron_schedule("b", "15 12 * * *", "UTC"), now)
        .await
        .unwrap();
    let due = store.next_due_time_excluding(&[]).await.unwrap().unwrap();
    assert_eq!(due, at(2026, 8, 4, 12, 15));
    let due = store
        .next_due_time_excluding(&[b.id])
        .await
        .unwrap()
        .unwrap();
    assert_eq!(due, at(2026, 8, 4, 12, 30));
    assert!(
        store
            .next_due_time_excluding(&[a.id, b.id])
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn awaiting_approval_fire_can_be_finished_as_denied() {
    let store = setup_store().await;
    let now = at(2026, 8, 4, 12, 0);
    let mut schedule = new_cron_schedule("gated", "0 9 * * *", "UTC");
    schedule.confirmation = ScheduleConfirmation::Confirm;
    let schedule = store.insert_schedule(&schedule, now).await.unwrap();
    let fire = schedule.next_run_at.unwrap();
    let claimed = store
        .claim_fire(schedule.id, fire, now, FireClaimMode::AwaitConfirmation)
        .await
        .unwrap()
        .unwrap();
    let runs = store.list_runs(schedule.id, 10).await.unwrap();
    assert_eq!(runs[0].status, ScheduleRunStatus::AwaitingApproval);
    store
        .finish_run(
            schedule.id,
            claimed.run_id,
            ScheduleRunStatus::Denied,
            None,
            now,
        )
        .await
        .unwrap();
    let runs = store.list_runs(schedule.id, 10).await.unwrap();
    assert_eq!(runs[0].status, ScheduleRunStatus::Denied);
    assert_eq!(runs[0].finished_at, Some(now));
    let schedule = store.get_schedule(schedule.id).await.unwrap().unwrap();
    assert_eq!(schedule.last_status, Some(ScheduleRunStatus::Denied));
}

#[tokio::test]
async fn schedule_runs_are_bounded_by_limit() {
    let store = setup_store().await;
    let now = at(2026, 8, 4, 12, 0);
    let schedule = store
        .insert_schedule(&new_cron_schedule("daily", "0 9 * * *", "UTC"), now)
        .await
        .unwrap();
    let mut fire = schedule.next_run_at.unwrap();
    for _ in 0..3 {
        store
            .claim_fire(schedule.id, fire, now, FireClaimMode::SkipBusy)
            .await
            .unwrap();
        fire = store
            .get_schedule(schedule.id)
            .await
            .unwrap()
            .unwrap()
            .next_run_at
            .unwrap();
    }
    let runs = store.list_runs(schedule.id, 2).await.unwrap();
    assert_eq!(runs.len(), 2);
}

#[test]
fn embedding_bytes_roundtrip() {
    let original = vec![1.0_f32, 0.5, -0.25, 0.0];
    let bytes = embedding_to_bytes(&original);
    let restored = bytes_to_embedding(&bytes);
    for (a, b) in original.iter().zip(restored.iter()) {
        assert!((a - b).abs() < 1e-7, "Mismatch: {a} != {b}");
    }
}

/// The cosine-similarity helpers take a typed [`EmbeddingCol`] rather than a
/// `&str` + allowlist, so only the two permitted columns are representable and
/// there is no `assert!`/panic path to guard them.
#[test]
fn embedding_col_renders_expected_sql() {
    assert_eq!(EmbeddingCol::Bare.as_sql(), "embedding");
    assert_eq!(
        EmbeddingCol::Qualified.as_sql(),
        "memory_embeddings.embedding"
    );

    // Both variants build an expression without panicking.
    let bytes = embedding_to_bytes(&[1.0_f32, 0.0, 0.0, 0.0]);
    let _ = cosine_similarity_expr(EmbeddingCol::Bare, &bytes);
    let _ = cosine_similarity_expr(EmbeddingCol::Qualified, &bytes);
    let _ = cosine_similarity_filter(EmbeddingCol::Qualified, &bytes, 0.5);
}

fn new_session_meta(session_id: &str, card_name: &str) -> crate::session::NewSessionMeta {
    crate::session::NewSessionMeta {
        session_id: session_id.to_string(),
        card_name: card_name.to_string(),
        title: format!("{session_id} title"),
    }
}

#[tokio::test]
async fn session_upsert_get_list_archive() {
    let store = setup_store().await;

    let id_a = store
        .upsert_session(&new_session_meta("sess-a", "card"))
        .await
        .unwrap();
    let id_b = store
        .upsert_session(&new_session_meta("sess-b", "card"))
        .await
        .unwrap();
    assert_ne!(id_a, id_b);

    let id_a2 = store
        .upsert_session(&new_session_meta("sess-a", "card"))
        .await
        .unwrap();
    assert_eq!(id_a, id_a2);

    store.touch_session("sess-a", 5).await.unwrap();
    let meta = store.get_session("sess-a").await.unwrap().unwrap();
    assert_eq!(meta.turn_count, 5);
    assert!(!meta.archived);

    store.set_session_archived("sess-b", true).await.unwrap();
    let active = store.list_sessions(false, 10).await.unwrap();
    assert_eq!(active.len(), 1);
    assert_eq!(active.first().unwrap().session_id, "sess-a");

    let all = store.list_sessions(true, 10).await.unwrap();
    assert_eq!(all.len(), 2);

    let updated = store.set_session_archived("nope", true).await.unwrap();
    assert!(!updated);
}

#[tokio::test]
async fn message_search_is_case_insensitive_and_paginated() {
    let store = setup_store().await;
    store
        .upsert_session(&new_session_meta("s1", "card"))
        .await
        .unwrap();
    store
        .upsert_session(&new_session_meta("s2", "card"))
        .await
        .unwrap();

    store
        .insert_log("s1", "card", "user", "Hello World")
        .await
        .unwrap();
    store
        .insert_log("s2", "card", "assistant", "hello there")
        .await
        .unwrap();
    store
        .insert_log("s1", "card", "user", "goodbye")
        .await
        .unwrap();

    let hits = store.search_messages("HELLO", 10, 0).await.unwrap();
    assert_eq!(hits.len(), 2);
    let sessions: std::collections::HashSet<&str> =
        hits.iter().map(|(sid, _)| sid.as_str()).collect();
    assert!(sessions.contains("s1"));
    assert!(sessions.contains("s2"));

    let page = store.search_messages("hello", 1, 0).await.unwrap();
    assert_eq!(page.len(), 1);
    let page2 = store.search_messages("hello", 1, 1).await.unwrap();
    assert_eq!(page2.len(), 1);

    let empty = store.search_messages("", 10, 0).await.unwrap();
    assert!(empty.is_empty());
}

#[tokio::test]
async fn message_search_escapes_like_metacharacters() {
    let store = setup_store().await;
    store
        .upsert_session(&new_session_meta("s1", "card"))
        .await
        .unwrap();

    store
        .insert_log("s1", "card", "user", "battery at 50% charge")
        .await
        .unwrap();
    store
        .insert_log("s1", "card", "user", "value is 500 units")
        .await
        .unwrap();
    store
        .insert_log("s1", "card", "user", "snake_case identifier")
        .await
        .unwrap();
    store
        .insert_log("s1", "card", "user", "snakeXcase near miss")
        .await
        .unwrap();
    store
        .insert_log("s1", "card", "user", r"path is C:\temp")
        .await
        .unwrap();

    // `%` must match literally: only the "50%" row, not "500" (which the
    // unescaped pattern `%50%%` would also match via the trailing wildcard).
    let pct = store.search_messages("50%", 10, 0).await.unwrap();
    assert_eq!(pct.len(), 1);
    assert!(pct[0].1.content.contains("50%"));

    // `_` must match a literal underscore, not "any single character".
    let underscore = store.search_messages("snake_case", 10, 0).await.unwrap();
    assert_eq!(underscore.len(), 1);
    assert!(underscore[0].1.content.contains("snake_case"));

    // The escape character itself is escaped: a literal backslash matches.
    let backslash = store.search_messages(r"C:\temp", 10, 0).await.unwrap();
    assert_eq!(backslash.len(), 1);
    assert!(backslash[0].1.content.contains(r"C:\temp"));
}

#[tokio::test]
async fn message_search_query_plan_uses_created_at_index() {
    use sea_orm::sea_query::{Expr, Func, LikeExpr};
    use sea_orm::{
        DbBackend, EntityTrait, ExprTrait, QueryFilter, QueryOrder, QuerySelect, QueryTrait,
        Statement,
    };

    let store = setup_store().await;
    store
        .upsert_session(&new_session_meta("s1", "card"))
        .await
        .unwrap();
    store
        .insert_log("s1", "card", "user", "hello world")
        .await
        .unwrap();

    // Build the exact query `search_messages` executes through SeaORM's own
    // query builder, so the EXPLAIN plan reflects the real SQL (the
    // hand-written string below it is kept only as documentation). The
    // leading-wildcard `LIKE` cannot use a B-tree index on `content` (that
    // requires FTS5), but the ordering must be served by
    // `idx_log_created_at` so the query walks rows newest-first and stops at
    // the limit instead of sorting the whole table.
    let built = crate::entities::conversation_logs::Entity::find()
        .filter(
            Func::lower(Expr::col(
                crate::entities::conversation_logs::Column::Content,
            ))
            .like(LikeExpr::new("%hello%".to_string()).escape('\\')),
        )
        .order_by_desc(crate::entities::conversation_logs::Column::CreatedAt)
        .limit(10)
        .offset(0)
        .build(DbBackend::Sqlite)
        .to_string();
    let explain_sql = format!("EXPLAIN QUERY PLAN {built}");

    let rows = store
        .connection()
        .query_all_raw(Statement::from_string(DbBackend::Sqlite, explain_sql))
        .await
        .unwrap();

    let plan: String = rows
        .iter()
        .map(|row| {
            row.try_get_by_index::<String>(3)
                .expect("EXPLAIN QUERY PLAN detail column exists")
        })
        .collect::<Vec<_>>()
        .join("\n");

    assert!(
        plan.contains("idx_log_created_at"),
        "expected the search ordering to use idx_log_created_at, plan was:\n{plan}"
    );
    assert!(
        !plan.to_ascii_lowercase().contains("use temp b-tree"),
        "expected no in-memory sort for the search ordering, plan was:\n{plan}"
    );
}

#[tokio::test]
async fn export_import_roundtrip_and_conflict() {
    let store = setup_store().await;
    store
        .upsert_session(&new_session_meta("orig", "card"))
        .await
        .unwrap();
    store
        .insert_log("orig", "card", "user", "my key is sk-secret123 bye")
        .await
        .unwrap();
    store
        .insert_log("orig", "card", "assistant", "noted")
        .await
        .unwrap();

    let export = store.build_export("orig").await.unwrap();
    assert_eq!(export.session.session_id, "orig");
    assert_eq!(export.messages.len(), 2);
    // Secrets in message content are redacted (order-independent).
    let joined: String = export.messages.iter().map(|m| m.content.as_str()).collect();
    assert!(joined.contains("[redacted]"));
    assert!(!joined.contains("sk-secret123"));

    let json = export.to_json().unwrap();
    let parsed = crate::export::SessionExport::from_json(&json).unwrap();
    assert_eq!(parsed.session.session_id, "orig");

    let new_id = store.import_export(&parsed).await.unwrap();
    let sessions = store.list_sessions(true, 10).await.unwrap();
    assert_eq!(sessions.len(), 2);
    let imported = sessions.iter().find(|s| s.id == new_id).unwrap();
    assert!(imported.session_id.starts_with("imported-"));
    let imported_msgs = store.list_messages(&imported.session_id).await.unwrap();
    assert_eq!(imported_msgs.len(), 2);

    let mut fresh = parsed.clone();
    fresh.session.session_id = "brand-new".to_string();
    store.import_export(&fresh).await.unwrap();
    assert!(store.get_session("brand-new").await.unwrap().is_some());
}

#[tokio::test]
async fn tool_embedding_field_upsert_overwrites_and_list_filters() {
    let store = setup_store().await;
    let emb = vec![1.0_f32, 0.0, 0.0, 0.0];

    store
        .upsert_tool_embedding_field(
            "web_search",
            "description",
            "",
            "hash-a",
            "",
            &emb,
            "desc text",
        )
        .await
        .unwrap();
    store
        .upsert_tool_embedding_field("web_search", "summary", "", "hash-a", "", &emb, "sum text")
        .await
        .unwrap();
    store
        .upsert_tool_embedding_field("web_search", "negative", "", "hash-a", "", &emb, "neg text")
        .await
        .unwrap();
    store
        .upsert_tool_embedding_field(
            "other_tool",
            "description",
            "",
            "hash-b",
            "",
            &emb,
            "other desc",
        )
        .await
        .unwrap();

    let rows = store.list_tool_embedding_fields().await.unwrap();
    assert_eq!(rows.len(), 4);
    let web_rows: Vec<_> = rows
        .iter()
        .filter(|r| r.tool_name == "web_search")
        .collect();
    assert_eq!(web_rows.len(), 3);
    let fields: std::collections::HashSet<&str> =
        web_rows.iter().map(|r| r.field.as_str()).collect();
    assert!(fields.contains("summary"));
    assert!(fields.contains("description"));
    assert!(fields.contains("negative"));

    let emb2 = vec![0.0_f32, 1.0, 0.0, 0.0];
    store
        .upsert_tool_embedding_field(
            "web_search",
            "summary",
            "",
            "hash-a2",
            "",
            &emb2,
            "sum text v2",
        )
        .await
        .unwrap();
    let rows = store.list_tool_embedding_fields().await.unwrap();
    let web_summary = rows
        .iter()
        .find(|r| r.tool_name == "web_search" && r.field == "summary")
        .unwrap();
    assert_eq!(web_summary.version_hash, "hash-a2");
    assert_eq!(web_summary.embedding, emb2);
    assert_eq!(web_summary.source_text, "sum text v2");
}

#[tokio::test]
async fn delete_tool_embeddings_cascades_to_fields() {
    let store = setup_store().await;
    let emb = vec![1.0_f32, 0.0, 0.0, 0.0];

    for field in ["summary", "description", "negative"] {
        store
            .upsert_tool_embedding_field("web_search", field, "", "hash", "", &emb, "text")
            .await
            .unwrap();
    }
    store
        .upsert_tool_embedding_field("keep_me", "description", "", "hash", "", &emb, "keep text")
        .await
        .unwrap();

    assert_eq!(store.list_tool_embedding_fields().await.unwrap().len(), 4);
    let deleted = store.delete_tool_embeddings("web_search").await.unwrap();
    assert_eq!(deleted, 3);
    assert_eq!(store.list_tool_embedding_fields().await.unwrap().len(), 1);
}

/// Regression test: embedding insert
/// must reject vectors whose length does not match
/// `embedding_dim` and vectors containing NaN /
/// Infinity, returning a typed `InvalidEmbedding`
/// error rather than letting the row be silently
/// persisted and poisoning later cosine queries.
#[tokio::test]
async fn upsert_memory_embedding_rejects_bad_embedding() {
    let store = setup_store().await;
    let id = store
        .insert_typed_memory(&crate::NewMemoryItem {
            scope: crate::MemoryScope::Character,
            character_id: "ene".into(),
            user_id: String::new(),
            kind: crate::MemoryKind::Semantic,
            title: "fact".into(),
            content: "content".into(),
            source: crate::MemorySource::Conversation,
            source_ref: None,
            confidence: crate::MemoryConfidence::default(),
            salience: crate::MemorySalience::default(),
            affect: crate::AffectAnnotation::default(),
            relationship_impact: 0.0,
            valid_from: None,
            valid_until: None,
            status: crate::MemoryStatus::Active,
            supersedes_id: None,
            pinned: false,
            created_at: None,
            commitment_id: None,
        })
        .await
        .unwrap();

    let wrong_len = vec![0.1, 0.2, 0.3];
    let err = store
        .upsert_memory_embedding(id, "test-model", "content", &wrong_len)
        .await
        .unwrap_err();
    assert!(
        matches!(err, EneMemoryError::InvalidEmbedding(_)),
        "expected InvalidEmbedding, got {err:?}"
    );

    let with_nan = vec![0.1, f32::NAN, 0.3, 0.4];
    let err = store
        .upsert_memory_embedding(id, "test-model", "content", &with_nan)
        .await
        .unwrap_err();
    assert!(
        matches!(err, EneMemoryError::InvalidEmbedding(_)),
        "expected InvalidEmbedding, got {err:?}"
    );

    let with_inf = vec![0.1, 0.2, f32::INFINITY, 0.4];
    let err = store
        .upsert_memory_embedding(id, "test-model", "content", &with_inf)
        .await
        .unwrap_err();
    assert!(
        matches!(err, EneMemoryError::InvalidEmbedding(_)),
        "expected InvalidEmbedding, got {err:?}"
    );

    let ok = vec![0.1, 0.2, 0.3, 0.4];
    store
        .upsert_memory_embedding(id, "test-model", "content", &ok)
        .await
        .unwrap();
}

/// Regression test: the memory store
/// must apply `foreign_keys=ON` (and the other
/// safety PRAGMAs) on every connection it opens. For
/// an in-memory store `journal_mode=WAL` is a no-op
/// (`SQLite` returns `memory`), but `foreign_keys` and
/// `busy_timeout` are still meaningful.
#[tokio::test]
async fn pragmas_are_applied_on_open() {
    use sea_orm::{ConnectionTrait, DbBackend, Statement};
    let store = setup_store().await;
    // Query the PRAGMA value directly instead of relying on rows_affected()
    // which is unreliable for PRAGMA statements.
    let row = store
        .connection()
        .query_one_raw(Statement::from_string(
            DbBackend::Sqlite,
            "PRAGMA foreign_keys".to_string(),
        ))
        .await
        .unwrap()
        .expect("PRAGMA foreign_keys should return a row");
    let value: i32 = row.try_get_by_index(0).unwrap();
    assert_eq!(value, 1, "foreign_keys PRAGMA should report 1 (ON)");
}

#[tokio::test]
async fn config_style_in_memory_path_survives_pool_use() {
    // `StoreConfig::resolve_memory_db_path` maps `in_memory: true` to the
    // `:memory:` path, which goes through `open_with_options` with a pool.
    // SQLite `:memory:` databases are per-connection, so the pool must be
    // capped at one or a later request hits a fresh empty DB ("no such
    // table"). Insert + read after the write forces a pool round-trip.
    let store = MemoryStore::open_with_options(
        std::path::Path::new(":memory:"),
        4,
        &crate::backup::OpenOptions::default(),
    )
    .await
    .expect("open :memory: via options path");
    store
        .insert_conversation_turn("pool-test", "ene", "Hello", "Hi there!")
        .await
        .expect("insert must hit the migrated in-memory DB");
    let logs = store
        .get_logs_by_session("pool-test")
        .await
        .expect("read must hit the same migrated DB");
    assert_eq!(logs.len(), 2);
}

#[tokio::test]
async fn insert_conversation_turn_stores_and_retrieves() {
    let store = setup_store().await;
    let session_id = "turn-test-session";

    let ids = store
        .insert_conversation_turn(session_id, "ene", "Hello", "Hi there!")
        .await
        .unwrap();
    let logs = store.get_logs_by_session(session_id).await.unwrap();
    assert_eq!(logs.len(), 2);
    let user_log = logs.first().expect("user log");
    let assistant_log = logs.get(1).expect("assistant log");
    assert_eq!(user_log.role, "user");
    assert_eq!(user_log.content, "Hello");
    assert_eq!(assistant_log.role, "assistant");
    assert_eq!(assistant_log.content, "Hi there!");
    let _ = ids;
}

#[tokio::test]
async fn affect_state_get_upsert() {
    let store = setup_store().await;

    let result = store.get_affect_state("ene").await.unwrap();
    assert!((result.valence - 0.0).abs() < f32::EPSILON);
    assert!((result.arousal - 0.0).abs() < f32::EPSILON);
    assert!(result.discrete_emotions.is_empty());

    let mut state = crate::AffectState {
        character_id: "ene".into(),
        user_id: String::new(),
        valence: 0.5,
        arousal: -0.3,
        dominance: 0.1,
        trust: 0.4,
        affinity: 0.6,
        irritation: 0.0,
        curiosity: 0.7,
        fatigue: 0.1,
        mood_label: String::new(),
        last_expression: String::new(),
        discrete_emotions: vec![
            crate::DiscreteEmotion::new("joy", 0.8),
            crate::DiscreteEmotion::new("surprise", 0.4),
        ],
        updated_at: None,
    };
    store.upsert_affect_state(&state).await.unwrap();

    let loaded = store.get_affect_state("ene").await.unwrap();
    assert!((loaded.valence - 0.5).abs() < f32::EPSILON);
    assert!((loaded.arousal + 0.3).abs() < f32::EPSILON);
    assert_eq!(loaded.discrete_emotions.len(), 2);
    let joy = loaded.discrete_emotions.first().expect("joy emotion");
    assert_eq!(joy.label, "joy");
    assert!((joy.intensity - 0.8).abs() < f32::EPSILON);

    state.valence = -0.2;
    state.discrete_emotions = vec![crate::DiscreteEmotion::new("sadness", 0.6)];
    store.upsert_affect_state(&state).await.unwrap();

    let loaded2 = store.get_affect_state("ene").await.unwrap();
    assert!((loaded2.valence + 0.2).abs() < f32::EPSILON);
    assert_eq!(loaded2.discrete_emotions.len(), 1);
    let sadness = loaded2.discrete_emotions.first().expect("sadness emotion");
    assert_eq!(sadness.label, "sadness");
}

#[tokio::test]
async fn typed_memory_crud() {
    let store = setup_store().await;

    let item = crate::NewMemoryItem {
        scope: crate::MemoryScope::Character,
        character_id: "ene".into(),
        user_id: String::new(),
        kind: crate::MemoryKind::Episodic,
        title: "Greeting".into(),
        content: "The user greeted me this morning".into(),
        source: crate::MemorySource::Conversation,
        source_ref: Some("sess-1/turn-1".into()),
        confidence: crate::MemoryConfidence::new(0.9),
        salience: crate::MemorySalience::new(0.5),
        affect: crate::AffectAnnotation::default(),
        relationship_impact: 0.0,
        valid_from: None,
        valid_until: None,
        status: crate::MemoryStatus::Active,
        supersedes_id: None,
        pinned: false,
        created_at: None,
        commitment_id: None,
    };

    let id = store.insert_typed_memory(&item).await.unwrap();
    assert!(id > 0);

    let loaded = store.get_typed_memory(id).await.unwrap().unwrap();
    assert_eq!(loaded.title, "Greeting");
    assert_eq!(loaded.kind, crate::MemoryKind::Episodic);
    assert!((loaded.confidence.get() - 0.9).abs() < f32::EPSILON);

    let by_char = store
        .get_typed_memories_by_character("ene", None, None, None, 10, 0)
        .await
        .unwrap();
    assert!(!by_char.is_empty());

    let count = store
        .count_typed_memories("ene", Some(crate::MemoryKind::Episodic))
        .await
        .unwrap();
    assert!(count > 0);

    let status_ok = store
        .set_memory_status(id, crate::MemoryStatus::Faded)
        .await
        .unwrap();
    assert!(status_ok);

    let access_ok = store.bump_typed_memory_access(id).await.unwrap();
    assert!(access_ok);

    let loaded2 = store.get_typed_memory(id).await.unwrap().unwrap();
    assert_eq!(loaded2.status, crate::MemoryStatus::Faded);
    assert_eq!(loaded2.access_count, 1);

    assert!(store.get_typed_memory(999_999).await.unwrap().is_none());
    assert!(
        !store
            .set_memory_status(999_999, crate::MemoryStatus::Active)
            .await
            .unwrap()
    );
    assert!(!store.bump_typed_memory_access(999_999).await.unwrap());
}

#[tokio::test]
async fn recent_tool_failures_lists_recallable_reflections_only() {
    let store = setup_store().await;

    let reflection = |title: &str, status: crate::MemoryStatus| crate::NewMemoryItem {
        scope: crate::MemoryScope::Shared,
        character_id: "ene".into(),
        user_id: String::new(),
        kind: crate::MemoryKind::Reflection,
        title: title.into(),
        content: format!("failure record for {title}"),
        source: crate::MemorySource::Inferred,
        source_ref: None,
        confidence: crate::MemoryConfidence::new(0.65),
        salience: crate::MemorySalience::default(),
        affect: crate::AffectAnnotation::default(),
        relationship_impact: 0.0,
        valid_from: None,
        valid_until: None,
        status,
        supersedes_id: None,
        pinned: false,
        created_at: None,
        commitment_id: None,
    };

    store
        .insert_typed_memory(&reflection("tool failure:web", crate::MemoryStatus::Active))
        .await
        .unwrap();
    store
        .insert_typed_memory(&reflection("tool failure:fs", crate::MemoryStatus::Active))
        .await
        .unwrap();
    // A superseded failure must not be reported.
    store
        .insert_typed_memory(&reflection(
            "tool failure:git",
            crate::MemoryStatus::Superseded,
        ))
        .await
        .unwrap();
    // A Faded failure is still recallable and keeps penalizing the tool:
    // fading is a decay signal for recall relevance, not an un-failure.
    store
        .insert_typed_memory(&reflection("tool failure:db", crate::MemoryStatus::Faded))
        .await
        .unwrap();
    // A non-Reflection memory with a matching title must not be reported.
    let mut semantic = reflection("tool failure:sqlite", crate::MemoryStatus::Active);
    semantic.kind = crate::MemoryKind::Semantic;
    store.insert_typed_memory(&semantic).await.unwrap();

    let failures = store.recent_tool_failures("ene").await.unwrap();
    assert_eq!(
        failures.len(),
        3,
        "only recallable Reflection failures: {failures:?}"
    );
    assert!(failures.contains(&"web".to_string()));
    assert!(failures.contains(&"fs".to_string()));
    assert!(failures.contains(&"db".to_string()));
    assert!(!failures.contains(&"git".to_string()));
    assert!(!failures.contains(&"sqlite".to_string()));
}

#[tokio::test]
async fn typed_memory_search_with_embedding() {
    let store = setup_store().await;

    let item = crate::NewMemoryItem {
        scope: crate::MemoryScope::Character,
        character_id: "ene".into(),
        user_id: String::new(),
        kind: crate::MemoryKind::Semantic,
        title: "Test memory".into(),
        content: "The user likes pizza".into(),
        source: crate::MemorySource::Conversation,
        source_ref: None,
        confidence: crate::MemoryConfidence::new(0.8),
        salience: crate::MemorySalience::new(0.6),
        affect: crate::AffectAnnotation::default(),
        relationship_impact: 0.0,
        valid_from: None,
        valid_until: None,
        status: crate::MemoryStatus::Active,
        supersedes_id: None,
        pinned: false,
        created_at: None,
        commitment_id: None,
    };

    let id = store.insert_typed_memory(&item).await.unwrap();
    let emb = vec![0.1, 0.2, 0.3, 0.4];
    store
        .upsert_memory_embedding(id, "test-model", "content", &emb)
        .await
        .unwrap();

    let results = store
        .search_typed_memories(&emb, "ene", "test-model", 10, 0.0)
        .await
        .unwrap();
    assert!(!results.is_empty());
    let top = results.first().expect("typed memory search result");
    assert!((top.1 - 1.0).abs() < f32::EPSILON);
    assert_eq!(top.0.title, "Test memory");
}

#[tokio::test]
async fn supersede_typed_memory_links_rows() {
    let store = setup_store().await;

    let old_item = crate::NewMemoryItem {
        scope: crate::MemoryScope::User,
        character_id: "ene".into(),
        user_id: "user1".into(),
        kind: crate::MemoryKind::Preference,
        title: "drink".into(),
        content: "likes coffee".into(),
        source: crate::MemorySource::Inferred,
        source_ref: None,
        confidence: crate::MemoryConfidence::new(0.7),
        salience: crate::MemorySalience::default(),
        affect: crate::AffectAnnotation::default(),
        relationship_impact: 0.0,
        valid_from: None,
        valid_until: None,
        status: crate::MemoryStatus::Active,
        supersedes_id: None,
        pinned: false,
        created_at: None,
        commitment_id: None,
    };
    let old_id = store.insert_typed_memory(&old_item).await.unwrap();

    let new_item = crate::NewMemoryItem {
        content: "likes tea".into(),
        confidence: crate::MemoryConfidence::new(0.9),
        ..old_item
    };
    let new_id = store
        .supersede_typed_memory(&new_item, old_id)
        .await
        .unwrap();

    let old = store.get_typed_memory(old_id).await.unwrap().unwrap();
    assert_eq!(old.status, crate::MemoryStatus::Superseded);
    assert_eq!(old.supersedes_id, None);

    let new_mem = store.get_typed_memory(new_id).await.unwrap().unwrap();
    assert_eq!(new_mem.supersedes_id, Some(old_id));
    assert_eq!(new_mem.status, crate::MemoryStatus::Active);
}

#[tokio::test]
async fn supersede_typed_memory_rejects_terminal_status() {
    let store = setup_store().await;

    let item = crate::NewMemoryItem {
        scope: crate::MemoryScope::User,
        character_id: "ene".into(),
        user_id: "user1".into(),
        kind: crate::MemoryKind::Preference,
        title: "drink".into(),
        content: "likes coffee".into(),
        source: crate::MemorySource::Inferred,
        source_ref: None,
        confidence: crate::MemoryConfidence::new(0.7),
        salience: crate::MemorySalience::default(),
        affect: crate::AffectAnnotation::default(),
        relationship_impact: 0.0,
        valid_from: None,
        valid_until: None,
        status: crate::MemoryStatus::Active,
        supersedes_id: None,
        pinned: false,
        created_at: None,
        commitment_id: None,
    };
    let old_id = store.insert_typed_memory(&item).await.unwrap();
    store
        .set_memory_status(old_id, crate::MemoryStatus::UserDeleted)
        .await
        .unwrap();

    let replacement = crate::NewMemoryItem {
        content: "likes tea".into(),
        ..item
    };
    let err = store
        .supersede_typed_memory(&replacement, old_id)
        .await
        .unwrap_err();
    assert!(
        matches!(err, EneMemoryError::Other(_)),
        "expected Other error, got {err:?}"
    );
}

#[tokio::test]
async fn commitment_crud_and_lifecycle() {
    let store = setup_store().await;

    let id = store
        .insert_commitment(&crate::NewCommitment {
            character_id: "ene".into(),
            user_id: "user1".into(),
            title: "design review".into(),
            description: "Discuss the design next session".into(),
            status: crate::CommitmentStatus::Active,
            due_at: None,
            due_label: Some("next session".into()),
        })
        .await
        .unwrap();

    let loaded = store.get_commitment(id).await.unwrap().unwrap();
    assert_eq!(loaded.title, "design review");

    let active = store
        .list_active_commitments("ene", Some("user1"), 10)
        .await
        .unwrap();
    assert_eq!(active.len(), 1);

    assert!(store.complete_commitment(id).await.unwrap());
    let done = store.get_commitment(id).await.unwrap().unwrap();
    assert_eq!(done.status, crate::CommitmentStatus::Done);
    assert!(done.completed_at.is_some());

    let active_after = store
        .list_active_commitments("ene", None, 10)
        .await
        .unwrap();
    assert!(active_after.is_empty());
}

#[tokio::test]
async fn mark_stale_commitments_past_due() {
    let store = setup_store().await;
    let past = Utc::now() - chrono::Duration::days(1);
    let future = Utc::now() + chrono::Duration::days(1);

    let stale_target = store
        .insert_commitment(&crate::NewCommitment {
            character_id: "ene".into(),
            user_id: String::new(),
            title: "overdue".into(),
            description: "was due yesterday".into(),
            status: crate::CommitmentStatus::Active,
            due_at: Some(past),
            due_label: None,
        })
        .await
        .unwrap();
    let _still_active = store
        .insert_commitment(&crate::NewCommitment {
            character_id: "ene".into(),
            user_id: String::new(),
            title: "upcoming".into(),
            description: "due tomorrow".into(),
            status: crate::CommitmentStatus::Active,
            due_at: Some(future),
            due_label: None,
        })
        .await
        .unwrap();

    let updated = store.mark_stale_commitments(Utc::now()).await.unwrap();
    assert_eq!(updated, 1);

    let stale = store.get_commitment(stale_target).await.unwrap().unwrap();
    assert_eq!(stale.status, crate::CommitmentStatus::Stale);
}

#[tokio::test]
async fn list_active_commitments_orders_dated_before_undated() {
    let store = setup_store().await;
    let now = Utc::now();

    let later_id = store
        .insert_commitment(&crate::NewCommitment {
            character_id: "ene".into(),
            user_id: "user1".into(),
            title: "later".into(),
            description: "due in two days".into(),
            status: crate::CommitmentStatus::Active,
            due_at: Some(now + chrono::Duration::days(2)),
            due_label: None,
        })
        .await
        .unwrap();
    let sooner_id = store
        .insert_commitment(&crate::NewCommitment {
            character_id: "ene".into(),
            user_id: "user1".into(),
            title: "sooner".into(),
            description: "due tomorrow".into(),
            status: crate::CommitmentStatus::Active,
            due_at: Some(now + chrono::Duration::days(1)),
            due_label: None,
        })
        .await
        .unwrap();
    let undated_id = store
        .insert_commitment(&crate::NewCommitment {
            character_id: "ene".into(),
            user_id: "user1".into(),
            title: "undated".into(),
            description: "no due date".into(),
            status: crate::CommitmentStatus::Active,
            due_at: None,
            due_label: Some("next time".into()),
        })
        .await
        .unwrap();

    let active = store
        .list_active_commitments("ene", Some("user1"), 10)
        .await
        .unwrap();
    let ids: Vec<i64> = active.iter().map(|c| c.id.unwrap()).collect();
    assert_eq!(ids, vec![sooner_id, later_id, undated_id]);
}

#[tokio::test]
async fn terminal_commitment_status_is_not_overwritten() {
    let store = setup_store().await;
    let past = Utc::now() - chrono::Duration::days(1);

    let done_id = store
        .insert_commitment(&crate::NewCommitment {
            character_id: "ene".into(),
            user_id: "user1".into(),
            title: "completed".into(),
            description: "already done".into(),
            status: crate::CommitmentStatus::Active,
            due_at: Some(past),
            due_label: None,
        })
        .await
        .unwrap();
    assert!(store.complete_commitment(done_id).await.unwrap());

    let updated = store.mark_stale_commitments(Utc::now()).await.unwrap();
    assert_eq!(updated, 0);

    let done = store.get_commitment(done_id).await.unwrap().unwrap();
    assert_eq!(done.status, crate::CommitmentStatus::Done);
    assert!(done.completed_at.is_some());

    assert!(!store.cancel_commitment(done_id).await.unwrap());
    assert!(
        !store
            .update_commitment_status(done_id, crate::CommitmentStatus::Stale)
            .await
            .unwrap()
    );
}

fn hybrid_search_options<'a>(
    query_text: &'a str,
    query_embedding: &'a [f32],
    now: DateTime<Utc>,
) -> crate::Query<'a> {
    crate::Query {
        query_text,
        embedding: Some(query_embedding),
        character_id: "ene",
        user_id: None,
        model_name: "test-model",
        limit: 10,
        similarity_threshold: 0.0,
        candidate_pool_size: 50,
        query_affect: None,
        weights: crate::HybridSearchWeights::default(),
        decay_half_life_days: 30.0,
        access_boost_half_life_days: 14.0,
        now,
        time_range: None,
        min_score: 0.0,
        commitment_boost: 0.25,
        recent_fallback_limit: 5,
        exclude_kinds: Vec::new(),
    }
}

async fn insert_memory_with_embedding(
    store: &MemoryStore,
    item: &crate::NewMemoryItem,
    embedding: &[f32],
) -> i64 {
    let id = store.insert_typed_memory(item).await.unwrap();
    store
        .upsert_memory_embedding(id, "test-model", "content", embedding)
        .await
        .unwrap();
    id
}

#[tokio::test]
async fn hybrid_search_ranks_by_salience_and_recency_not_vector_alone() {
    let store = setup_store().await;
    let now = Utc::now();
    let query_emb = vec![1.0, 0.0, 0.0, 0.0];

    let low_salience = crate::NewMemoryItem {
        scope: crate::MemoryScope::Character,
        character_id: "ene".into(),
        user_id: String::new(),
        kind: crate::MemoryKind::Semantic,
        title: "distant topic".into(),
        content: "unrelated content".into(),
        source: crate::MemorySource::Conversation,
        source_ref: None,
        confidence: crate::MemoryConfidence::new(0.5),
        salience: crate::MemorySalience::new(0.2),
        affect: crate::AffectAnnotation::default(),
        relationship_impact: 0.0,
        valid_from: None,
        valid_until: None,
        status: crate::MemoryStatus::Active,
        supersedes_id: None,
        pinned: false,
        created_at: None,
        commitment_id: None,
    };
    let high_salience = crate::NewMemoryItem {
        salience: crate::MemorySalience::new(0.95),
        confidence: crate::MemoryConfidence::new(0.9),
        title: "important fact".into(),
        content: "user preference about music".into(),
        ..low_salience.clone()
    };

    insert_memory_with_embedding(&store, &low_salience, &query_emb).await;
    insert_memory_with_embedding(&store, &high_salience, &query_emb).await;

    let options = hybrid_search_options("music preference", &query_emb, now);
    let gathered = store.search(&options).await.unwrap();
    let results = ene_rag::score_and_rank(&options, gathered);
    assert_eq!(results.len(), 2);
    let top = results.first().expect("top hybrid result");
    let second = results.get(1).expect("second hybrid result");
    assert_eq!(top.item.title, "important fact");
    assert!(top.breakdown.total >= second.breakdown.total);
}

#[tokio::test]
async fn hybrid_search_exclude_kinds_drops_reflection_memories() {
    let store = setup_store().await;
    let now = Utc::now();
    let query_emb = vec![1.0, 0.0, 0.0, 0.0];

    let normal = test_memory_item("favorite food", "the user likes pizza");
    let reflection = crate::NewMemoryItem {
        kind: crate::MemoryKind::Reflection,
        title: "Successful strategies".into(),
        content: "Successful interaction strategies: favorite food".into(),
        ..test_memory_item("base", "base")
    };

    insert_memory_with_embedding(&store, &normal, &query_emb).await;
    insert_memory_with_embedding(&store, &reflection, &query_emb).await;

    let mut options = hybrid_search_options("pizza", &query_emb, now);
    let gathered = store.search(&options).await.unwrap();
    let results = ene_rag::score_and_rank(&options, gathered);
    assert!(
        results
            .iter()
            .any(|m| m.item.kind == crate::MemoryKind::Reflection),
        "reflection memory is gathered when not excluded"
    );

    options.exclude_kinds = vec![crate::MemoryKind::Reflection];
    let gathered = store.search(&options).await.unwrap();
    let results = ene_rag::score_and_rank(&options, gathered);
    assert!(
        results
            .iter()
            .all(|m| m.item.kind != crate::MemoryKind::Reflection),
        "excluded kinds must not appear in recall candidates"
    );
    assert!(
        results.iter().any(|m| m.item.title == "favorite food"),
        "normal memories are unaffected by the reflection exclusion"
    );
}

#[tokio::test]
async fn hybrid_search_lexical_component_for_matching_query() {
    let store = setup_store().await;
    let now = Utc::now();
    let orthogonal = vec![0.0, 1.0, 0.0, 0.0];

    let item = crate::NewMemoryItem {
        scope: crate::MemoryScope::Character,
        character_id: "ene".into(),
        user_id: String::new(),
        kind: crate::MemoryKind::Semantic,
        title: "favorite drink".into(),
        content: "The user loves matcha latte".into(),
        source: crate::MemorySource::Conversation,
        source_ref: None,
        confidence: crate::MemoryConfidence::default(),
        salience: crate::MemorySalience::default(),
        affect: crate::AffectAnnotation::default(),
        relationship_impact: 0.0,
        valid_from: None,
        valid_until: None,
        status: crate::MemoryStatus::Active,
        supersedes_id: None,
        pinned: false,
        created_at: None,
        commitment_id: None,
    };
    insert_memory_with_embedding(&store, &item, &orthogonal).await;

    let options = hybrid_search_options("matcha latte", &orthogonal, now);
    let gathered = store.search(&options).await.unwrap();
    let results = ene_rag::score_and_rank(&options, gathered);
    assert_eq!(results.len(), 1);
    let result = results.first().expect("lexical match result");
    assert!(result.breakdown.lexical_score > 0.0);
    assert!(
        result
            .sources
            .contains(&crate::MemoryCandidateSource::Lexical)
    );
}

#[tokio::test]
async fn hybrid_search_surfaces_active_commitment_with_low_vector_similarity() {
    let store = setup_store().await;
    let now = Utc::now();
    let query_emb = vec![1.0, 0.0, 0.0, 0.0];

    let commitment_id = store
        .insert_commitment(&crate::NewCommitment {
            character_id: "ene".into(),
            user_id: "user1".into(),
            title: "follow up".into(),
            description: "Review the architecture document".into(),
            status: crate::CommitmentStatus::Active,
            due_at: None,
            due_label: Some("next time".into()),
        })
        .await
        .unwrap();

    let options = hybrid_search_options("unrelated query", &query_emb, now);
    let gathered = store.search(&options).await.unwrap();
    let results = ene_rag::score_and_rank(&options, gathered);
    assert!(
        results.iter().any(|r| {
            r.item.commitment_id == Some(commitment_id)
                && r.breakdown.commitment_boost > 0.0
                && r.sources
                    .contains(&crate::MemoryCandidateSource::Commitment)
        }),
        "active ledger commitment should be recalled despite low vector similarity"
    );
}

#[tokio::test]
async fn hybrid_search_excludes_archived_superseded_and_user_deleted() {
    let store = setup_store().await;
    let now = Utc::now();
    let emb = vec![0.5, 0.5, 0.5, 0.5];

    let base = crate::NewMemoryItem {
        scope: crate::MemoryScope::Character,
        character_id: "ene".into(),
        user_id: String::new(),
        kind: crate::MemoryKind::Semantic,
        title: "memory".into(),
        content: "shared content".into(),
        source: crate::MemorySource::Conversation,
        source_ref: None,
        confidence: crate::MemoryConfidence::default(),
        salience: crate::MemorySalience::default(),
        affect: crate::AffectAnnotation::default(),
        relationship_impact: 0.0,
        valid_from: None,
        valid_until: None,
        status: crate::MemoryStatus::Active,
        supersedes_id: None,
        pinned: false,
        created_at: None,
        commitment_id: None,
    };

    for status in [
        crate::MemoryStatus::Archived,
        crate::MemoryStatus::Superseded,
        crate::MemoryStatus::UserDeleted,
    ] {
        let mut item = base.clone();
        item.title = format!("{status:?}");
        item.status = status;
        insert_memory_with_embedding(&store, &item, &emb).await;
    }

    let options = hybrid_search_options("shared content", &emb, now);
    let gathered = store.search(&options).await.unwrap();
    let results = ene_rag::score_and_rank(&options, gathered);
    assert!(results.is_empty());
}

#[tokio::test]
async fn hybrid_search_faded_memory_has_stale_penalty() {
    let store = setup_store().await;
    let now = Utc::now();
    let emb = vec![1.0, 0.0, 0.0, 0.0];

    let item = crate::NewMemoryItem {
        scope: crate::MemoryScope::Character,
        character_id: "ene".into(),
        user_id: String::new(),
        kind: crate::MemoryKind::Semantic,
        title: "old fact".into(),
        content: "faded memory content".into(),
        source: crate::MemorySource::Conversation,
        source_ref: None,
        confidence: crate::MemoryConfidence::default(),
        salience: crate::MemorySalience::default(),
        affect: crate::AffectAnnotation::default(),
        relationship_impact: 0.0,
        valid_from: None,
        valid_until: None,
        status: crate::MemoryStatus::Faded,
        supersedes_id: None,
        pinned: false,
        created_at: None,
        commitment_id: None,
    };
    insert_memory_with_embedding(&store, &item, &emb).await;

    let options = hybrid_search_options("faded memory", &emb, now);
    let gathered = store.search(&options).await.unwrap();
    let results = ene_rag::score_and_rank(&options, gathered);
    assert_eq!(results.len(), 1);
    let result = results.first().expect("faded memory result");
    assert!(result.breakdown.vector_similarity > 0.0);
    assert!(result.breakdown.stale_penalty > 0.0);
}

#[tokio::test]
async fn hybrid_search_finds_old_lexical_match_outside_recent_pool() {
    let store = setup_store().await;
    let now = Utc::now();
    let query_emb = vec![0.0, 0.0, 1.0, 0.0];
    let orthogonal = vec![0.0, 1.0, 0.0, 0.0];

    let old_item = crate::NewMemoryItem {
        scope: crate::MemoryScope::Character,
        character_id: "ene".into(),
        user_id: String::new(),
        kind: crate::MemoryKind::Semantic,
        title: "ancient dragon recipe".into(),
        content: "A very old note about ancient dragon recipe".into(),
        source: crate::MemorySource::Conversation,
        source_ref: None,
        confidence: crate::MemoryConfidence::default(),
        salience: crate::MemorySalience::default(),
        affect: crate::AffectAnnotation::default(),
        relationship_impact: 0.0,
        valid_from: None,
        valid_until: None,
        status: crate::MemoryStatus::Active,
        supersedes_id: None,
        pinned: false,
        created_at: None,
        commitment_id: None,
    };
    let old_id = store.insert_typed_memory(&old_item).await.unwrap();
    store
        .upsert_memory_embedding(old_id, "test-model", "content", &orthogonal)
        .await
        .unwrap();

    for i in 0..10 {
        let filler = crate::NewMemoryItem {
            scope: crate::MemoryScope::Character,
            character_id: "ene".into(),
            user_id: String::new(),
            kind: crate::MemoryKind::Semantic,
            title: format!("recent filler {i}"),
            content: "unrelated filler content".into(),
            source: crate::MemorySource::Conversation,
            source_ref: None,
            confidence: crate::MemoryConfidence::default(),
            salience: crate::MemorySalience::new(0.95),
            affect: crate::AffectAnnotation::default(),
            relationship_impact: 0.0,
            valid_from: None,
            valid_until: None,
            status: crate::MemoryStatus::Active,
            supersedes_id: None,
            pinned: false,
            created_at: None,
            commitment_id: None,
        };
        insert_memory_with_embedding(&store, &filler, &orthogonal).await;
    }

    let mut options = hybrid_search_options("ancient dragon recipe", &query_emb, now);
    options.recent_fallback_limit = 0;
    options.similarity_threshold = 0.8;
    let gathered = store.search(&options).await.unwrap();
    let results = ene_rag::score_and_rank(&options, gathered);
    assert!(
        results
            .iter()
            .any(|r| r.item.id == Some(old_id) && r.breakdown.lexical_score > 0.0),
        "old lexical match should be found outside the recent pool"
    );
}

#[tokio::test]
async fn hybrid_search_excludes_unrelated_recent_without_fallback() {
    let store = setup_store().await;
    let now = Utc::now();
    let query_emb = vec![1.0, 0.0, 0.0, 0.0];
    let orthogonal = vec![0.0, 1.0, 0.0, 0.0];

    let unrelated = crate::NewMemoryItem {
        scope: crate::MemoryScope::Character,
        character_id: "ene".into(),
        user_id: String::new(),
        kind: crate::MemoryKind::Semantic,
        title: "fresh but unrelated".into(),
        content: "nothing to do with the query".into(),
        source: crate::MemorySource::Conversation,
        source_ref: None,
        confidence: crate::MemoryConfidence::default(),
        salience: crate::MemorySalience::new(0.99),
        affect: crate::AffectAnnotation::default(),
        relationship_impact: 0.0,
        valid_from: None,
        valid_until: None,
        status: crate::MemoryStatus::Active,
        supersedes_id: None,
        pinned: false,
        created_at: None,
        commitment_id: None,
    };
    insert_memory_with_embedding(&store, &unrelated, &orthogonal).await;

    let mut options = hybrid_search_options("completely different topic", &query_emb, now);
    options.recent_fallback_limit = 0;
    options.similarity_threshold = 0.8;
    let gathered = store.search(&options).await.unwrap();
    let results = ene_rag::score_and_rank(&options, gathered);
    assert!(results.is_empty());
}

#[tokio::test]
async fn hybrid_search_ranks_higher_confidence_when_other_signals_match() {
    let store = setup_store().await;
    let now = Utc::now();
    let query_emb = vec![1.0, 0.0, 0.0, 0.0];

    let base = crate::NewMemoryItem {
        scope: crate::MemoryScope::Character,
        character_id: "ene".into(),
        user_id: String::new(),
        kind: crate::MemoryKind::Semantic,
        title: "shared topic".into(),
        content: "shared topic content".into(),
        source: crate::MemorySource::Conversation,
        source_ref: None,
        confidence: crate::MemoryConfidence::new(0.2),
        salience: crate::MemorySalience::new(0.5),
        affect: crate::AffectAnnotation::default(),
        relationship_impact: 0.0,
        valid_from: None,
        valid_until: None,
        status: crate::MemoryStatus::Active,
        supersedes_id: None,
        pinned: false,
        created_at: None,
        commitment_id: None,
    };
    let low_confidence = base.clone();
    let high_confidence = crate::NewMemoryItem {
        title: "shared topic high".into(),
        confidence: crate::MemoryConfidence::new(0.95),
        ..base
    };

    insert_memory_with_embedding(&store, &low_confidence, &query_emb).await;
    insert_memory_with_embedding(&store, &high_confidence, &query_emb).await;

    let options = hybrid_search_options("shared topic", &query_emb, now);
    let gathered = store.search(&options).await.unwrap();
    let results = ene_rag::score_and_rank(&options, gathered);
    assert_eq!(results.len(), 2);
    let top = results.first().expect("top confidence result");
    let second = results.get(1).expect("second confidence result");
    assert_eq!(top.item.title, "shared topic high");
    assert!(top.breakdown.confidence > second.breakdown.confidence);
}

#[tokio::test]
async fn hybrid_search_respects_user_id_scope() {
    let store = setup_store().await;
    let now = Utc::now();
    let query_emb = vec![1.0, 0.0, 0.0, 0.0];

    let base = crate::NewMemoryItem {
        scope: crate::MemoryScope::User,
        character_id: "ene".into(),
        user_id: String::new(),
        kind: crate::MemoryKind::Semantic,
        title: "scoped memory".into(),
        content: "user scoped content".into(),
        source: crate::MemorySource::Conversation,
        source_ref: None,
        confidence: crate::MemoryConfidence::default(),
        salience: crate::MemorySalience::default(),
        affect: crate::AffectAnnotation::default(),
        relationship_impact: 0.0,
        valid_from: None,
        valid_until: None,
        status: crate::MemoryStatus::Active,
        supersedes_id: None,
        pinned: false,
        created_at: None,
        commitment_id: None,
    };

    let mut user1_item = base.clone();
    user1_item.user_id = "user1".into();
    let mut user2_item = base;
    user2_item.user_id = "user2".into();

    insert_memory_with_embedding(&store, &user1_item, &query_emb).await;
    insert_memory_with_embedding(&store, &user2_item, &query_emb).await;

    let mut options = hybrid_search_options("scoped memory", &query_emb, now);
    options.user_id = Some("user1");
    let gathered = store.search(&options).await.unwrap();
    let results = ene_rag::score_and_rank(&options, gathered);
    assert_eq!(results.len(), 1);
    let result = results.first().expect("scoped search result");
    assert_eq!(result.item.user_id, "user1");
}

#[tokio::test]
async fn hybrid_search_dedupes_multi_source_candidates() {
    let store = setup_store().await;
    let now = Utc::now();
    let emb = vec![1.0, 0.0, 0.0, 0.0];

    let item = crate::NewMemoryItem {
        scope: crate::MemoryScope::Character,
        character_id: "ene".into(),
        user_id: String::new(),
        kind: crate::MemoryKind::Semantic,
        title: "pizza night".into(),
        content: "Friday pizza tradition".into(),
        source: crate::MemorySource::Conversation,
        source_ref: None,
        confidence: crate::MemoryConfidence::default(),
        salience: crate::MemorySalience::default(),
        affect: crate::AffectAnnotation::default(),
        relationship_impact: 0.0,
        valid_from: None,
        valid_until: None,
        status: crate::MemoryStatus::Active,
        supersedes_id: None,
        pinned: false,
        created_at: None,
        commitment_id: None,
    };
    insert_memory_with_embedding(&store, &item, &emb).await;

    let options = hybrid_search_options("pizza tradition", &emb, now);
    let gathered = store.search(&options).await.unwrap();
    let results = ene_rag::score_and_rank(&options, gathered);
    assert_eq!(results.len(), 1);
    let result = results.first().expect("deduped search result");
    assert!(result.sources.len() >= 2);
}

#[tokio::test]
async fn set_memory_status_rejects_invalid_edge() {
    let store = setup_store().await;
    let id = store
        .insert_typed_memory(&crate::NewMemoryItem {
            scope: crate::MemoryScope::Character,
            character_id: "ene".into(),
            user_id: String::new(),
            kind: crate::MemoryKind::Semantic,
            title: "fact".into(),
            content: "content".into(),
            source: crate::MemorySource::Conversation,
            source_ref: None,
            confidence: crate::MemoryConfidence::default(),
            salience: crate::MemorySalience::default(),
            affect: crate::AffectAnnotation::default(),
            relationship_impact: 0.0,
            valid_from: None,
            valid_until: None,
            status: crate::MemoryStatus::Faded,
            supersedes_id: None,
            pinned: false,
            created_at: None,
            commitment_id: None,
        })
        .await
        .unwrap();

    let err = store
        .set_memory_status(id, crate::MemoryStatus::Active)
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        EneMemoryError::InvalidTransition {
            from: crate::MemoryStatus::Faded,
            to: crate::MemoryStatus::Active,
        }
    ));
}

#[tokio::test]
async fn apply_natural_decay_batch_fades_and_archives() {
    let store = setup_store().await;
    let now = Utc::now();

    let active_id = store
        .insert_typed_memory(&crate::NewMemoryItem {
            scope: crate::MemoryScope::Character,
            character_id: "ene".into(),
            user_id: "user1".into(),
            kind: crate::MemoryKind::Semantic,
            title: "old active".into(),
            content: "old content".into(),
            source: crate::MemorySource::Conversation,
            source_ref: None,
            confidence: crate::MemoryConfidence::new(0.5),
            salience: crate::MemorySalience::new(0.5),
            affect: crate::AffectAnnotation::default(),
            relationship_impact: 0.0,
            valid_from: None,
            valid_until: None,
            status: crate::MemoryStatus::Active,
            supersedes_id: None,
            pinned: false,
            created_at: None,
            commitment_id: None,
        })
        .await
        .unwrap();
    store
        .test_backdate_typed_memory(active_id, 30)
        .await
        .unwrap();

    let faded_id = store
        .insert_typed_memory(&crate::NewMemoryItem {
            scope: crate::MemoryScope::Character,
            character_id: "ene".into(),
            user_id: "user1".into(),
            kind: crate::MemoryKind::Semantic,
            title: "old faded".into(),
            content: "very old content".into(),
            source: crate::MemorySource::Conversation,
            source_ref: None,
            confidence: crate::MemoryConfidence::new(0.1),
            salience: crate::MemorySalience::new(0.1),
            affect: crate::AffectAnnotation::default(),
            relationship_impact: 0.0,
            valid_from: None,
            valid_until: None,
            status: crate::MemoryStatus::Faded,
            supersedes_id: None,
            pinned: false,
            created_at: None,
            commitment_id: None,
        })
        .await
        .unwrap();
    store
        .test_backdate_typed_memory(faded_id, 365)
        .await
        .unwrap();

    let report = store
        .apply_natural_decay_batch(
            "ene",
            Some("user1"),
            now,
            30.0,
            ene_rag::FADE_THRESHOLD,
            ene_rag::ARCHIVE_THRESHOLD,
        )
        .await
        .unwrap();
    assert!(report.faded_count >= 1);
    assert!(report.archived_count >= 1);

    let active_loaded = store.get_typed_memory(active_id).await.unwrap().unwrap();
    assert_eq!(active_loaded.status, crate::MemoryStatus::Faded);

    let faded_loaded = store.get_typed_memory(faded_id).await.unwrap().unwrap();
    assert_eq!(faded_loaded.status, crate::MemoryStatus::Archived);
}

#[tokio::test]
async fn apply_natural_decay_batch_ignores_non_finite_params() {
    let store = setup_store().await;
    let id = store
        .insert_typed_memory(&crate::NewMemoryItem {
            scope: crate::MemoryScope::Character,
            character_id: "ene".into(),
            user_id: "user1".into(),
            kind: crate::MemoryKind::Semantic,
            title: "stale".into(),
            content: "should stay active when params are non-finite".into(),
            source: crate::MemorySource::Conversation,
            source_ref: None,
            confidence: crate::MemoryConfidence::new(0.1),
            salience: crate::MemorySalience::new(0.1),
            affect: crate::AffectAnnotation::default(),
            relationship_impact: 0.0,
            valid_from: None,
            valid_until: None,
            status: crate::MemoryStatus::Active,
            supersedes_id: None,
            pinned: false,
            created_at: None,
            commitment_id: None,
        })
        .await
        .unwrap();
    store.test_backdate_typed_memory(id, 200).await.unwrap();

    let now = Utc::now();
    for (half_life, fade, archive) in [
        (
            f64::INFINITY,
            ene_rag::FADE_THRESHOLD,
            ene_rag::ARCHIVE_THRESHOLD,
        ),
        (30.0, f32::NAN, ene_rag::ARCHIVE_THRESHOLD),
        (30.0, ene_rag::FADE_THRESHOLD, f32::NEG_INFINITY),
    ] {
        let report = store
            .apply_natural_decay_batch("ene", Some("user1"), now, half_life, fade, archive)
            .await
            .unwrap();
        assert_eq!(report.faded_count, 0);
        assert_eq!(report.archived_count, 0);
    }

    let loaded = store.get_typed_memory(id).await.unwrap().unwrap();
    assert_eq!(loaded.status, crate::MemoryStatus::Active);
}

/// Rust [`ene_rag::decay_score`] and the SQL expression used by natural decay
/// must agree on the same row (shared coefficients from `ene-rag`).
#[tokio::test]
async fn natural_decay_sql_matches_rust_decay_score() {
    use sea_orm::{ConnectionTrait, DatabaseBackend, Statement};

    let store = setup_store().await;
    let now = Utc::now();
    let half_life_days = 30.0_f64;
    let id = store
        .insert_typed_memory(&crate::NewMemoryItem {
            scope: crate::MemoryScope::Character,
            character_id: "ene".into(),
            user_id: "user1".into(),
            kind: crate::MemoryKind::Semantic,
            title: "score-match".into(),
            content: "compare rust vs sql decay".into(),
            source: crate::MemorySource::Conversation,
            source_ref: None,
            confidence: crate::MemoryConfidence::new(0.6),
            salience: crate::MemorySalience::new(0.7),
            affect: crate::AffectAnnotation {
                valence: 0.4,
                arousal: -0.2,
            },
            relationship_impact: 0.0,
            valid_from: None,
            valid_until: None,
            status: crate::MemoryStatus::Active,
            supersedes_id: None,
            pinned: false,
            created_at: None,
            commitment_id: None,
        })
        .await
        .unwrap();
    store.test_backdate_typed_memory(id, 45).await.unwrap();

    let item = store.get_typed_memory(id).await.unwrap().unwrap();
    let rust_score = ene_rag::decay_score(&item, now, half_life_days);

    let now_text = now.format("%Y-%m-%d %H:%M:%S").to_string();
    let score_sql = memory::natural_decay_score_sql("updated_at");
    let sql = format!("SELECT ({score_sql}) AS score FROM typed_memories WHERE id = ?");
    let row = store
        .connection()
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Sqlite,
            sql,
            [half_life_days.into(), now_text.into(), id.into()],
        ))
        .await
        .unwrap()
        .expect("row");
    let sql_score: f64 = row.try_get("", "score").unwrap();

    assert!(
        (f64::from(rust_score) - sql_score).abs() < 1e-5,
        "rust={rust_score} sql={sql_score}"
    );
}

#[tokio::test]
async fn pin_typed_memory_excludes_from_natural_decay() {
    let store = setup_store().await;
    let id = store
        .insert_typed_memory(&crate::NewMemoryItem {
            scope: crate::MemoryScope::Character,
            character_id: "ene".into(),
            user_id: String::new(),
            kind: crate::MemoryKind::Semantic,
            title: "pinned".into(),
            content: "pinned content".into(),
            source: crate::MemorySource::Conversation,
            source_ref: None,
            confidence: crate::MemoryConfidence::new(0.1),
            salience: crate::MemorySalience::new(0.1),
            affect: crate::AffectAnnotation::default(),
            relationship_impact: 0.0,
            valid_from: None,
            valid_until: None,
            status: crate::MemoryStatus::Active,
            supersedes_id: None,
            pinned: true,
            created_at: None,
            commitment_id: None,
        })
        .await
        .unwrap();
    store.test_backdate_typed_memory(id, 200).await.unwrap();

    let report = store
        .apply_natural_decay_batch(
            "ene",
            None,
            Utc::now(),
            30.0,
            ene_rag::FADE_THRESHOLD,
            ene_rag::ARCHIVE_THRESHOLD,
        )
        .await
        .unwrap();
    assert_eq!(report.faded_count, 0);

    let loaded = store.get_typed_memory(id).await.unwrap().unwrap();
    assert_eq!(loaded.status, crate::MemoryStatus::Active);
    assert!(loaded.pinned);
}

/// SQL decay processes the full table in one pass (no `BATCH_LIMIT` cap).
#[tokio::test]
async fn apply_natural_decay_sql_handles_large_table_in_one_pass() {
    let store = setup_store().await;
    let now = Utc::now();
    for i in 0..300 {
        let id = store
            .insert_typed_memory(&crate::NewMemoryItem {
                scope: crate::MemoryScope::Character,
                character_id: "ene".into(),
                user_id: "user1".into(),
                kind: crate::MemoryKind::Semantic,
                title: format!("bulk-{i}"),
                content: "stale bulk row".into(),
                source: crate::MemorySource::Conversation,
                source_ref: None,
                confidence: crate::MemoryConfidence::new(0.1),
                salience: crate::MemorySalience::new(0.1),
                affect: crate::AffectAnnotation::default(),
                relationship_impact: 0.0,
                valid_from: None,
                valid_until: None,
                status: crate::MemoryStatus::Active,
                supersedes_id: None,
                pinned: false,
                created_at: None,
                commitment_id: None,
            })
            .await
            .unwrap();
        store.test_backdate_typed_memory(id, 120).await.unwrap();
    }

    let report = store
        .apply_natural_decay_batch(
            "ene",
            Some("user1"),
            now,
            30.0,
            ene_rag::FADE_THRESHOLD,
            ene_rag::ARCHIVE_THRESHOLD,
        )
        .await
        .unwrap();
    assert!(
        report.faded_count >= 300,
        "expected all 300 stale rows to fade in a single SQL pass, got {}",
        report.faded_count
    );
}

#[tokio::test]
async fn transition_active_to_faded_sets_faded_at_from_decay_anchor() {
    let store = setup_store().await;
    let id = store
        .insert_typed_memory(&crate::NewMemoryItem {
            scope: crate::MemoryScope::Character,
            character_id: "ene".into(),
            user_id: "user1".into(),
            kind: crate::MemoryKind::Semantic,
            title: "anchor test".into(),
            content: "anchor content".into(),
            source: crate::MemorySource::Conversation,
            source_ref: None,
            confidence: crate::MemoryConfidence::default(),
            salience: crate::MemorySalience::default(),
            affect: crate::AffectAnnotation::default(),
            relationship_impact: 0.0,
            valid_from: None,
            valid_until: None,
            status: crate::MemoryStatus::Active,
            supersedes_id: None,
            pinned: false,
            created_at: None,
            commitment_id: None,
        })
        .await
        .unwrap();
    store.test_backdate_typed_memory(id, 45).await.unwrap();

    let before = store.get_typed_memory(id).await.unwrap().unwrap();
    assert!(
        store
            .set_memory_status(id, crate::MemoryStatus::Faded)
            .await
            .unwrap()
    );

    let after = store.get_typed_memory(id).await.unwrap().unwrap();
    assert_eq!(after.status, crate::MemoryStatus::Faded);
    assert_eq!(after.faded_at, Some(before.updated_at));
}

#[tokio::test]
async fn single_row_natural_decay_reaches_archived_in_two_passes() {
    use chrono::Duration as ChronoDuration;

    let store = setup_store().await;
    let now = Utc::now();

    let id = store
        .insert_typed_memory(&crate::NewMemoryItem {
            scope: crate::MemoryScope::Character,
            character_id: "ene".into(),
            user_id: "user1".into(),
            kind: crate::MemoryKind::Semantic,
            title: "ancient".into(),
            content: "very old fact".into(),
            source: crate::MemorySource::Conversation,
            source_ref: None,
            confidence: crate::MemoryConfidence::new(0.5),
            salience: crate::MemorySalience::new(0.5),
            affect: crate::AffectAnnotation::default(),
            relationship_impact: 0.0,
            valid_from: None,
            valid_until: None,
            status: crate::MemoryStatus::Active,
            supersedes_id: None,
            pinned: false,
            created_at: None,
            commitment_id: None,
        })
        .await
        .unwrap();
    store.test_backdate_typed_memory(id, 30).await.unwrap();

    let first = store
        .apply_natural_decay_batch(
            "ene",
            Some("user1"),
            now,
            30.0,
            ene_rag::FADE_THRESHOLD,
            ene_rag::ARCHIVE_THRESHOLD,
        )
        .await
        .unwrap();
    assert_eq!(first.faded_count, 1);
    assert_eq!(first.archived_count, 0);

    let faded = store.get_typed_memory(id).await.unwrap().unwrap();
    assert_eq!(faded.status, crate::MemoryStatus::Faded);
    assert!(faded.faded_at.is_some());

    let later = now + ChronoDuration::days(200);
    let second = store
        .apply_natural_decay_batch(
            "ene",
            Some("user1"),
            later,
            30.0,
            ene_rag::FADE_THRESHOLD,
            ene_rag::ARCHIVE_THRESHOLD,
        )
        .await
        .unwrap();
    assert_eq!(second.archived_count, 1);

    let archived = store.get_typed_memory(id).await.unwrap().unwrap();
    assert_eq!(archived.status, crate::MemoryStatus::Archived);
}

#[tokio::test]
async fn hybrid_search_preserves_pinned_flag() {
    let store = setup_store().await;
    let now = Utc::now();
    let emb = vec![1.0, 0.0, 0.0, 0.0];

    let item = crate::NewMemoryItem {
        scope: crate::MemoryScope::Character,
        character_id: "ene".into(),
        user_id: String::new(),
        kind: crate::MemoryKind::Semantic,
        title: "pinned fact".into(),
        content: "pinned vector content".into(),
        source: crate::MemorySource::Conversation,
        source_ref: None,
        confidence: crate::MemoryConfidence::default(),
        salience: crate::MemorySalience::default(),
        affect: crate::AffectAnnotation::default(),
        relationship_impact: 0.0,
        valid_from: None,
        valid_until: None,
        status: crate::MemoryStatus::Active,
        supersedes_id: None,
        pinned: true,
        created_at: None,
        commitment_id: None,
    };
    insert_memory_with_embedding(&store, &item, &emb).await;

    let options = hybrid_search_options("pinned vector", &emb, now);
    let gathered = store.search(&options).await.unwrap();
    let results = ene_rag::score_and_rank(&options, gathered);
    assert_eq!(results.len(), 1);
    let result = results.first().expect("pinned search result");
    assert!(result.item.pinned);
}

#[tokio::test]
async fn file_backup_restore_and_integrity_roundtrip() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("memory.db");
    let store = MemoryStore::open(&path, 4).await.expect("open");
    store
        .insert_log("s1", "card", "user", "hello")
        .await
        .expect("insert");
    store.check_integrity().await.expect("integrity");
    let backup = store.backup().await.expect("backup");
    drop(store);

    // Corrupt the live DB, then restore from the backup.
    std::fs::write(&path, b"not-a-sqlite-database").expect("corrupt");
    crate::backup::restore_database(&backup, &path).expect("restore");
    let store = MemoryStore::open(&path, 4).await.expect("reopen");
    store
        .check_integrity()
        .await
        .expect("integrity after restore");
    let logs = store.get_logs_by_session("s1").await.expect("logs");
    assert_eq!(logs.len(), 1);
}

#[tokio::test]
async fn schema_too_new_is_reported() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("memory.db");
    let store = MemoryStore::open(&path, 4).await.expect("open");
    // Inject an unknown migration name into seaql_migrations.
    store
        .connection()
        .execute_unprepared(
            "INSERT INTO seaql_migrations (version, applied_at) VALUES ('m20990101_future', 0)",
        )
        .await
        .expect("inject");
    drop(store);

    let err = MemoryStore::open(&path, 4)
        .await
        .err()
        .expect("expected SchemaTooNew");
    assert!(
        matches!(err, EneMemoryError::SchemaTooNew { .. }),
        "unexpected error: {err}"
    );
}

#[tokio::test]
async fn restore_after_simulated_migration_failure_keeps_db_usable() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("memory.db");
    let store = MemoryStore::open(&path, 4).await.expect("open");
    store
        .insert_log("s1", "card", "user", "keep-me")
        .await
        .expect("insert");
    let backup = store.backup().await.expect("backup");
    drop(store);

    // Simulate a half-applied migration by wiping the live file, then
    // restoring the backup the way open_with_options does on failure.
    std::fs::write(&path, b"").expect("wipe");
    crate::backup::restore_database(&backup, &path).expect("restore");
    let store = MemoryStore::open(&path, 4).await.expect("reopen");
    let logs = store.get_logs_by_session("s1").await.expect("logs");
    assert_eq!(logs.len(), 1);
    assert_eq!(
        logs.first().map(|entry| entry.content.as_str()),
        Some("keep-me")
    );
}

#[tokio::test]
async fn pending_memory_write_queue_roundtrip() {
    let store = setup_store().await;
    let id = store
        .enqueue_pending_memory_write("ene", "user", r#"{"character_id":"ene"}"#, "boom")
        .await
        .expect("enqueue");
    let (pending, permanent) = store
        .count_pending_memory_writes("ene")
        .await
        .expect("count");
    assert_eq!(pending, 1);
    assert_eq!(permanent, 0);

    // Freshly enqueued rows wait 30s before becoming due, so `take_due` would
    // return nothing here; listing and completing proves the queue CRUD instead.
    let listed = store
        .list_pending_memory_writes("ene", 10)
        .await
        .expect("list");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed.first().map(|r| r.id), Some(id));

    store
        .complete_pending_memory_write(id)
        .await
        .expect("complete");
    let (pending, _) = store
        .count_pending_memory_writes("ene")
        .await
        .expect("count after");
    assert_eq!(pending, 0);
}

#[test]
fn strip_tags_footer_removes_trailing_footer() {
    let content = "User likes coffee.\n\n<!-- ene:tags {\"tags\":[\"interrupted\"]} -->";
    assert_eq!(strip_tags_footer(content), "User likes coffee.");
}

#[test]
fn strip_tags_footer_leaves_content_without_footer() {
    let content = "User likes coffee.";
    assert_eq!(strip_tags_footer(content), "User likes coffee.");
}

#[test]
fn strip_tags_footer_handles_empty_content() {
    assert_eq!(strip_tags_footer(""), "");
}

#[test]
fn strip_tags_footer_preserves_mid_content_marker() {
    // "ene:tags" appearing mid-content (without the `\n\n<!-- ` prefix)
    // is not a footer and must be preserved.
    let content = "Discussed ene:tags format in the meeting.";
    assert_eq!(
        strip_tags_footer(content),
        "Discussed ene:tags format in the meeting."
    );
}

#[test]
fn strip_tags_footer_multiline_content() {
    let content = "Line one.\nLine two.\n\n<!-- ene:tags {\"tags\":[\"a\",\"b\"]} -->";
    assert_eq!(strip_tags_footer(content), "Line one.\nLine two.");
}

fn sample_pending_candidate(character_id: &str, title: &str) -> PendingCandidate {
    PendingCandidate {
        id: 0,
        character_id: character_id.to_string(),
        user_id: "user1".to_string(),
        title: title.to_string(),
        content: format!("{title} content"),
        kind: crate::MemoryKind::Preference,
        confidence: 0.8,
        reason_detail: "extracted from conversation".to_string(),
        existing_memory_title: None,
        existing_memory_id: None,
        source_quote: "I like tea".to_string(),
        status: PendingCandidateStatus::Pending,
        created_at: Utc::now(),
    }
}

#[tokio::test]
async fn pending_candidate_insert_list_approve_persists_typed_memory() {
    let store = setup_store().await;

    let id = store
        .insert_pending_candidate(sample_pending_candidate("ene", "likes tea"))
        .await
        .expect("insert");
    assert_eq!(id, 1);

    let listed = store
        .list_pending_candidates("ene", Some(PendingCandidateStatus::Pending))
        .await
        .expect("list");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].title, "likes tea");
    assert_eq!(listed[0].status, PendingCandidateStatus::Pending);

    let memory_id = store.approve_pending_candidate(id).await.expect("approve");
    let memory = store
        .get_typed_memory(memory_id)
        .await
        .expect("get memory")
        .expect("memory exists");
    assert_eq!(memory.title, "likes tea");
    assert_eq!(memory.kind, crate::MemoryKind::Preference);
    assert_eq!(memory.status, crate::MemoryStatus::Active);

    let pending = store
        .list_pending_candidates("ene", Some(PendingCandidateStatus::Pending))
        .await
        .expect("list pending");
    assert!(pending.is_empty());
    let approved = store
        .list_pending_candidates("ene", Some(PendingCandidateStatus::Approved))
        .await
        .expect("list approved");
    assert_eq!(approved.len(), 1);

    assert!(store.approve_pending_candidate(id).await.is_err());
}

#[tokio::test]
async fn pending_candidate_reject_does_not_persist() {
    let store = setup_store().await;

    let id = store
        .insert_pending_candidate(sample_pending_candidate("ene", "rejected fact"))
        .await
        .expect("insert");
    store
        .resolve_pending_candidate(id, false)
        .await
        .expect("reject");

    let rejected = store
        .list_pending_candidates("ene", Some(PendingCandidateStatus::Rejected))
        .await
        .expect("list rejected");
    assert_eq!(rejected.len(), 1);

    let count = store
        .count_typed_memories("ene", None)
        .await
        .expect("count");
    assert_eq!(count, 0);
}

/// Candidates persist to the `pending_candidates` table, so they
/// survive a restart (a fresh store instance on the same file) and are
/// shared across instances. Listing remains isolated per character.
#[tokio::test]
async fn pending_candidates_persist_across_instances_and_isolate_per_character() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("memory.db");

    let store = MemoryStore::open(&path, 4).await.expect("open");
    store
        .insert_pending_candidate(sample_pending_candidate("ene", "ene fact"))
        .await
        .expect("insert ene");
    store
        .insert_pending_candidate(sample_pending_candidate("mira", "mira fact"))
        .await
        .expect("insert mira");
    drop(store);

    let reopened = MemoryStore::open(&path, 4).await.expect("reopen");

    let ene = reopened
        .list_pending_candidates("ene", None)
        .await
        .expect("list ene");
    assert_eq!(ene.len(), 1);
    assert_eq!(ene[0].title, "ene fact");

    let mira = reopened
        .list_pending_candidates("mira", None)
        .await
        .expect("list mira");
    assert_eq!(mira.len(), 1);
    assert_eq!(mira[0].title, "mira fact");
}

// ── Pending-candidate retention policy ──

#[tokio::test]
async fn pending_candidate_age_prune_removes_expired() {
    let store = setup_store().await;

    let old_id = store
        .insert_pending_candidate(sample_pending_candidate("ene", "stale"))
        .await
        .expect("insert old");
    store
        .test_backdate_pending_candidate(old_id, 30)
        .await
        .expect("backdate");
    store
        .insert_pending_candidate(sample_pending_candidate("ene", "fresh"))
        .await
        .expect("insert fresh");

    let removed = store
        .prune_pending_candidates("ene", Some("user1"), 14, 0, Utc::now())
        .await
        .expect("prune");
    assert_eq!(removed, 1);

    let remaining = store
        .list_pending_candidates("ene", None)
        .await
        .expect("list");
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].title, "fresh");
}

#[tokio::test]
async fn pending_candidate_count_prune_caps_queue() {
    let store = setup_store().await;

    for i in 0..5 {
        store
            .insert_pending_candidate(sample_pending_candidate("ene", &format!("cand {i}")))
            .await
            .expect("insert");
    }

    let removed = store
        .prune_pending_candidates("ene", Some("user1"), 0, 3, Utc::now())
        .await
        .expect("prune");
    assert_eq!(removed, 2);

    let remaining = store
        .list_pending_candidates("ene", Some(PendingCandidateStatus::Pending))
        .await
        .expect("list");
    assert_eq!(remaining.len(), 3);
    assert_eq!(remaining[0].title, "cand 2");
}

#[tokio::test]
async fn pending_candidate_prune_disabled_with_zero() {
    let store = setup_store().await;

    let id = store
        .insert_pending_candidate(sample_pending_candidate("ene", "ancient"))
        .await
        .expect("insert");
    store
        .test_backdate_pending_candidate(id, 365)
        .await
        .expect("backdate");

    let removed = store
        .prune_pending_candidates("ene", Some("user1"), 0, 0, Utc::now())
        .await
        .expect("prune");
    assert_eq!(removed, 0);

    let remaining = store
        .list_pending_candidates("ene", None)
        .await
        .expect("list");
    assert_eq!(remaining.len(), 1);
}

/// Build a sample candidate for a specific user, optionally recording a
/// conflicting existing memory.
fn sample_pending_candidate_for(
    character_id: &str,
    user_id: &str,
    title: &str,
    existing_memory_id: Option<i64>,
) -> PendingCandidate {
    PendingCandidate {
        id: 0,
        character_id: character_id.to_string(),
        user_id: user_id.to_string(),
        title: title.to_string(),
        content: format!("{title} content"),
        kind: crate::MemoryKind::Preference,
        confidence: 0.8,
        reason_detail: "extracted from conversation".to_string(),
        existing_memory_title: None,
        existing_memory_id,
        source_quote: "I like tea".to_string(),
        status: PendingCandidateStatus::Pending,
        created_at: Utc::now(),
    }
}

/// Pruning is character-scoped — pruning "ene" must leave
/// "mira"'s queue untouched.
#[tokio::test]
async fn pending_candidate_prune_is_character_scoped() {
    let store = setup_store().await;

    for i in 0..5 {
        store
            .insert_pending_candidate(sample_pending_candidate("ene", &format!("ene {i}")))
            .await
            .expect("insert ene");
    }
    store
        .insert_pending_candidate(sample_pending_candidate("mira", "mira 0"))
        .await
        .expect("insert mira");

    let removed = store
        .prune_pending_candidates("ene", Some("user1"), 0, 2, Utc::now())
        .await
        .expect("prune");
    assert_eq!(removed, 3);

    assert_eq!(
        store
            .list_pending_candidates("ene", None)
            .await
            .expect("list ene")
            .len(),
        2
    );
    assert_eq!(
        store
            .list_pending_candidates("mira", None)
            .await
            .expect("list mira")
            .len(),
        1
    );
}

/// The count cap is scoped to the acting user (plus
/// character-shared rows), so one user's overflow cannot evict another's.
#[tokio::test]
async fn pending_candidate_prune_count_cap_is_user_scoped() {
    let store = setup_store().await;

    for i in 0..5 {
        store
            .insert_pending_candidate(sample_pending_candidate_for(
                "ene",
                "alice",
                &format!("alice {i}"),
                None,
            ))
            .await
            .expect("insert alice");
    }
    for i in 0..2 {
        store
            .insert_pending_candidate(sample_pending_candidate_for(
                "ene",
                "bob",
                &format!("bob {i}"),
                None,
            ))
            .await
            .expect("insert bob");
    }

    let removed = store
        .prune_pending_candidates("ene", Some("alice"), 0, 2, Utc::now())
        .await
        .expect("prune");
    assert_eq!(removed, 3);

    let all = store
        .list_pending_candidates("ene", None)
        .await
        .expect("list");
    let alice_rows = all.iter().filter(|c| c.user_id == "alice").count();
    let bob_rows = all.iter().filter(|c| c.user_id == "bob").count();
    assert_eq!(alice_rows, 2);
    assert_eq!(bob_rows, 2);
}

/// The age sweep removes *resolved* (approved / rejected) rows
/// too, not just pending ones — resolved rows have no further UI value once
/// stale.
#[tokio::test]
async fn pending_candidate_age_prune_removes_resolved_rows() {
    let store = setup_store().await;

    let approved_id = store
        .insert_pending_candidate(sample_pending_candidate("ene", "approved stale"))
        .await
        .expect("insert approved");
    store
        .approve_pending_candidate(approved_id)
        .await
        .expect("approve");
    let rejected_id = store
        .insert_pending_candidate(sample_pending_candidate("ene", "rejected stale"))
        .await
        .expect("insert rejected");
    store
        .resolve_pending_candidate(rejected_id, false)
        .await
        .expect("reject");

    store
        .test_backdate_pending_candidate(approved_id, 30)
        .await
        .expect("backdate approved");
    store
        .test_backdate_pending_candidate(rejected_id, 30)
        .await
        .expect("backdate rejected");
    store
        .insert_pending_candidate(sample_pending_candidate("ene", "fresh pending"))
        .await
        .expect("insert fresh");

    let removed = store
        .prune_pending_candidates("ene", Some("user1"), 14, 0, Utc::now())
        .await
        .expect("prune");
    assert_eq!(removed, 2);

    let remaining = store
        .list_pending_candidates("ene", None)
        .await
        .expect("list");
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].title, "fresh pending");
}

/// With both limits active, `removed` must not double-count a row
/// the age pass already deleted when the count pass runs afterwards.
#[tokio::test]
async fn pending_candidate_prune_both_limits_no_double_count() {
    let store = setup_store().await;

    for i in 0..2 {
        let id = store
            .insert_pending_candidate(sample_pending_candidate("ene", &format!("ancient {i}")))
            .await
            .expect("insert ancient");
        store
            .test_backdate_pending_candidate(id, 30)
            .await
            .expect("backdate");
    }
    for i in 0..4 {
        store
            .insert_pending_candidate(sample_pending_candidate("ene", &format!("fresh {i}")))
            .await
            .expect("insert fresh");
    }

    // Age pass removes the 2 ancient rows; the count pass then sees 4 fresh
    // pending rows and caps to 3, removing 1 more. Total = 3, not 4.
    let removed = store
        .prune_pending_candidates("ene", Some("user1"), 14, 3, Utc::now())
        .await
        .expect("prune");
    assert_eq!(removed, 3);

    let remaining = store
        .list_pending_candidates("ene", None)
        .await
        .expect("list");
    assert_eq!(remaining.len(), 3);
}

/// Approving a candidate that recorded a conflicting
/// existing memory propagates it to the typed memory's `supersedes_id`.
#[tokio::test]
async fn pending_candidate_approve_propagates_existing_memory_id() {
    let store = setup_store().await;

    let id = store
        .insert_pending_candidate(sample_pending_candidate_for(
            "ene",
            "user1",
            "supersedes old",
            Some(42),
        ))
        .await
        .expect("insert");

    let memory_id = store.approve_pending_candidate(id).await.expect("approve");
    let memory = store
        .get_typed_memory(memory_id)
        .await
        .expect("get memory")
        .expect("memory exists");
    assert_eq!(memory.supersedes_id, Some(42));
}

/// Resolving a candidate across a simulated restart —
/// insert on one instance, then approve / reject on a fresh instance over the
/// same file, which the shared-queue design promises.
#[tokio::test]
async fn pending_candidate_resolve_across_restart() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("memory.db");

    let store = MemoryStore::open(&path, 4).await.expect("open");
    let approve_id = store
        .insert_pending_candidate(sample_pending_candidate("ene", "approve me"))
        .await
        .expect("insert approve");
    let reject_id = store
        .insert_pending_candidate(sample_pending_candidate("ene", "reject me"))
        .await
        .expect("insert reject");
    drop(store);

    let reopened = MemoryStore::open(&path, 4).await.expect("reopen");
    let memory_id = reopened
        .approve_pending_candidate(approve_id)
        .await
        .expect("approve across restart");
    let memory = reopened
        .get_typed_memory(memory_id)
        .await
        .expect("get memory")
        .expect("memory exists");
    assert_eq!(memory.title, "approve me");

    reopened
        .resolve_pending_candidate(reject_id, false)
        .await
        .expect("reject across restart");

    let approved = reopened
        .list_pending_candidates("ene", Some(PendingCandidateStatus::Approved))
        .await
        .expect("list approved");
    assert_eq!(approved.len(), 1);
    let rejected = reopened
        .list_pending_candidates("ene", Some(PendingCandidateStatus::Rejected))
        .await
        .expect("list rejected");
    assert_eq!(rejected.len(), 1);
    let pending = reopened
        .list_pending_candidates("ene", Some(PendingCandidateStatus::Pending))
        .await
        .expect("list pending");
    assert!(pending.is_empty());
}

/// Concurrent approvals of the same candidate must insert
/// exactly one typed memory — one call wins the conditional status flip, the
/// other errors.
#[tokio::test]
async fn pending_candidate_concurrent_approve_inserts_once() {
    let store = std::sync::Arc::new(setup_store().await);

    let id = store
        .insert_pending_candidate(sample_pending_candidate("ene", "contested"))
        .await
        .expect("insert");

    let mut handles = Vec::new();
    for _ in 0..8 {
        let store = std::sync::Arc::clone(&store);
        handles.push(tokio::spawn(async move {
            store.approve_pending_candidate(id).await
        }));
    }

    let mut successes = 0usize;
    for handle in handles {
        let result = handle.await.expect("join");
        if result.is_ok() {
            successes += 1;
        }
    }
    assert_eq!(successes, 1, "exactly one concurrent approval may win");

    let count = store
        .count_typed_memories("ene", None)
        .await
        .expect("count");
    assert_eq!(count, 1);

    let approved = store
        .list_pending_candidates("ene", Some(PendingCandidateStatus::Approved))
        .await
        .expect("list approved");
    assert_eq!(approved.len(), 1);
}

// ── PRAGMAs must reach every pooled connection ──

/// The per-connection PRAGMAs must set
/// `foreign_keys=ON` (and the other safety PRAGMAs) on **every**
/// connection in the pool, not just the first one. A file-backed
/// store uses a pool of eight connections; we deterministically
/// exercise all of them by opening eight concurrent transactions
/// (each holds a distinct pool connection) behind a barrier so
/// they are all live simultaneously before checking the PRAGMA.
#[tokio::test]
async fn pragmas_apply_to_all_pool_connections() {
    use sea_orm::{ConnectionTrait, DbBackend, Statement, TransactionTrait};
    use std::sync::Arc;
    use tokio::sync::Barrier;

    const POOL_SIZE: usize = 8;

    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("memory.db");
    let store = MemoryStore::open(&path, 4).await.expect("open");

    let barrier = Arc::new(Barrier::new(POOL_SIZE));
    let mut handles = Vec::with_capacity(POOL_SIZE);
    for _ in 0..POOL_SIZE {
        let barrier = Arc::clone(&barrier);
        let db = store.connection().clone();
        handles.push(tokio::spawn(async move {
            let txn = db.begin().await.expect("begin txn");
            barrier.wait().await;
            let row = txn
                .query_one_raw(Statement::from_string(
                    DbBackend::Sqlite,
                    "PRAGMA foreign_keys".to_string(),
                ))
                .await
                .expect("query PRAGMA foreign_keys")
                .expect("row");
            let fk_on: i32 = row.try_get_by_index(0).expect("value");
            txn.commit().await.expect("commit");
            fk_on
        }));
    }

    for handle in handles {
        let fk_on = handle.await.expect("task panicked");
        assert_eq!(
            fk_on, 1,
            "foreign_keys must report 1 (ON) for every pool connection"
        );
    }
}

// ── FK cascade and unique index correctness ──

/// Builds a minimal [`crate::NewMemoryItem`] for test use.
fn test_memory_item(title: &str, content: &str) -> crate::NewMemoryItem {
    crate::NewMemoryItem {
        scope: crate::MemoryScope::Character,
        character_id: "ene".into(),
        user_id: String::new(),
        kind: crate::MemoryKind::Semantic,
        title: title.into(),
        content: content.into(),
        source: crate::MemorySource::Conversation,
        source_ref: None,
        confidence: crate::MemoryConfidence::default(),
        salience: crate::MemorySalience::default(),
        affect: crate::AffectAnnotation::default(),
        relationship_impact: 0.0,
        valid_from: None,
        valid_until: None,
        status: crate::MemoryStatus::Active,
        supersedes_id: None,
        pinned: false,
        created_at: None,
        commitment_id: None,
    }
}

/// Deleting a `typed_memories` row must cascade-delete its
/// `memory_embeddings` rows now that `foreign_keys=ON` is
/// enforced on every pooled connection.
#[tokio::test]
async fn delete_typed_memory_cascades_to_embeddings() {
    use sea_orm::{ColumnTrait, ConnectionTrait, EntityTrait, PaginatorTrait, QueryFilter};

    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("memory.db");
    let store = MemoryStore::open(&path, 4).await.expect("open");

    let item = test_memory_item("cascade test", "content to embed");
    let id = store.insert_typed_memory(&item).await.expect("insert");
    store
        .upsert_memory_embedding(id, "test-model", "content", &[0.1, 0.2, 0.3, 0.4])
        .await
        .expect("upsert embedding");

    let count_before = crate::entities::memory_embeddings::Entity::find()
        .filter(crate::entities::memory_embeddings::Column::MemoryItemId.eq(id))
        .count(store.connection())
        .await
        .expect("count before");
    assert_eq!(count_before, 1, "embedding must exist before delete");

    // Delete the parent row directly; the FK ON DELETE CASCADE must fire.
    store
        .connection()
        .execute_unprepared(&format!("DELETE FROM typed_memories WHERE id = {id}"))
        .await
        .expect("delete parent");

    let count_after = crate::entities::memory_embeddings::Entity::find()
        .filter(crate::entities::memory_embeddings::Column::MemoryItemId.eq(id))
        .count(store.connection())
        .await
        .expect("count after");
    assert_eq!(count_after, 0, "embedding must be cascade-deleted");
}

/// The unique index `uniq_memory_embedding` must prevent duplicate
/// rows for the same `(memory_item_id, model_name, field)` triple.
/// A second upsert must update the existing row, not insert a new one.
#[tokio::test]
async fn upsert_memory_embedding_does_not_create_duplicates() {
    use sea_orm::{DbBackend, Statement};

    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("memory.db");
    let store = MemoryStore::open(&path, 4).await.expect("open");

    let item = test_memory_item("dedup test", "content to embed");
    let id = store.insert_typed_memory(&item).await.expect("insert");

    let emb1: Vec<f32> = vec![0.1, 0.2, 0.3, 0.4];
    store
        .upsert_memory_embedding(id, "test-model", "content", &emb1)
        .await
        .expect("first upsert");

    let emb2: Vec<f32> = vec![0.5, 0.6, 0.7, 0.8];
    store
        .upsert_memory_embedding(id, "test-model", "content", &emb2)
        .await
        .expect("second upsert (update, not insert)");

    let rows = store
        .connection()
        .query_all_raw(Statement::from_string(
            DbBackend::Sqlite,
            format!(
                "SELECT embedding FROM memory_embeddings \
                 WHERE memory_item_id = {id} AND model_name = 'test-model' AND field = 'content'"
            ),
        ))
        .await
        .expect("query embeddings");

    assert_eq!(
        rows.len(),
        1,
        "unique index must prevent duplicate (memory_item_id, model_name, field)"
    );

    let stored_bytes: Vec<u8> = rows[0].try_get_by_index(0).expect("embedding bytes");
    assert_eq!(
        stored_bytes,
        embedding_to_bytes(&emb2),
        "stored embedding must be the second (updated) value"
    );
}

// ── vec0 ANN index tests ──

/// The vec0 indexed search must return the same results as the brute-force
/// scan for a small dataset where both paths are exhaustive.
#[tokio::test]
async fn vec0_search_matches_brute_force() {
    let store = setup_store().await;

    let items = [
        (
            "tea preferences",
            "user likes green tea",
            vec![1.0, 0.0, 0.0, 0.0],
        ),
        (
            "movie night",
            "user enjoys sci-fi movies",
            vec![0.8, 0.6, 0.0, 0.0],
        ),
        (
            "work schedule",
            "user works 9 to 5",
            vec![0.5, 0.5, 0.5, 0.5],
        ),
        (
            "pet cat",
            "user has a cat named Luna",
            vec![0.0, 0.0, 0.0, 1.0],
        ),
    ];

    for (title, content, emb) in &items {
        let item = test_memory_item(title, content);
        let id = store.insert_typed_memory(&item).await.unwrap();
        store
            .upsert_memory_embedding(id, "test-model", "content", emb)
            .await
            .unwrap();
    }

    let query = vec![0.9, 0.1, 0.0, 0.0];
    let statuses: &[&str] = &["active"];

    let indexed = store
        .search_typed_memories_vector(&query, "ene", "test-model", None, statuses, 10, 0.0)
        .await
        .unwrap();

    let brute = store
        .search_typed_memories_vector_brute_force(
            &query,
            "ene",
            "test-model",
            None,
            statuses,
            10,
            0.0,
        )
        .await
        .unwrap();

    assert_eq!(
        indexed.len(),
        brute.len(),
        "indexed and brute-force must return the same number of results"
    );

    for (i, (b_item, b_sim)) in brute.iter().enumerate() {
        let (i_item, i_sim) = &indexed[i];
        assert_eq!(i_item.id, b_item.id, "result {i}: same memory id expected");
        assert!(
            (i_sim - b_sim).abs() < 1e-5,
            "result {i}: similarity mismatch: indexed={i_sim}, brute={b_sim}"
        );
    }
}

/// The vec0 index must be kept in sync when embeddings are upserted
/// (insert and update paths).
#[tokio::test]
async fn vec0_sync_on_upsert() {
    let store = setup_store().await;

    let item = test_memory_item("sync test", "content for sync");
    let id = store.insert_typed_memory(&item).await.unwrap();

    let emb1 = vec![1.0, 0.0, 0.0, 0.0];
    store
        .upsert_memory_embedding(id, "test-model", "content", &emb1)
        .await
        .unwrap();

    let results = store
        .search_typed_memories_vector(&emb1, "ene", "test-model", None, &["active"], 10, 0.0)
        .await
        .unwrap();
    assert_eq!(results.len(), 1, "vec0 must contain the inserted embedding");
    assert_eq!(results[0].0.id, Some(id));

    let emb2 = vec![0.0, 1.0, 0.0, 0.0];
    store
        .upsert_memory_embedding(id, "test-model", "content", &emb2)
        .await
        .unwrap();

    // Search with the old vector should still find the row (it's the only one).
    let results = store
        .search_typed_memories_vector(&emb1, "ene", "test-model", None, &["active"], 10, 0.0)
        .await
        .unwrap();
    assert_eq!(
        results.len(),
        1,
        "vec0 must still contain exactly one row after update"
    );

    let results_new = store
        .search_typed_memories_vector(&emb2, "ene", "test-model", None, &["active"], 10, 0.0)
        .await
        .unwrap();
    assert_eq!(results_new.len(), 1);
    assert!(
        results_new[0].1 > results[0].1,
        "updated embedding must be more similar to the new vector"
    );
}

/// Deleting a typed memory (FK cascade) must remove the vec0 row.
#[tokio::test]
async fn vec0_sync_on_cascade_delete() {
    use sea_orm::ConnectionTrait;

    let store = setup_store().await;

    let item = test_memory_item("cascade vec0", "content to delete");
    let id = store.insert_typed_memory(&item).await.unwrap();
    store
        .upsert_memory_embedding(id, "test-model", "content", &[1.0, 0.0, 0.0, 0.0])
        .await
        .unwrap();

    let before = store
        .search_typed_memories_vector(
            &[1.0, 0.0, 0.0, 0.0],
            "ene",
            "test-model",
            None,
            &["active"],
            10,
            0.0,
        )
        .await
        .unwrap();
    assert_eq!(before.len(), 1, "vec0 row must exist before delete");

    // Delete the parent typed_memories row; FK cascade deletes memory_embeddings,
    // and the AFTER DELETE trigger removes the vec0 row.
    store
        .connection()
        .execute_unprepared(&format!("DELETE FROM typed_memories WHERE id = {id}"))
        .await
        .unwrap();

    let after = store
        .search_typed_memories_vector(
            &[1.0, 0.0, 0.0, 0.0],
            "ene",
            "test-model",
            None,
            &["active"],
            10,
            0.0,
        )
        .await
        .unwrap();
    assert_eq!(
        after.len(),
        0,
        "vec0 row must be removed after cascade delete"
    );
}

/// The vec0 tool embedding index must be kept in sync with
/// `tool_embedding_index` writes and deletes.
#[tokio::test]
async fn vec0_tool_search_sync() {
    let store = setup_store().await;

    let emb = vec![1.0, 0.0, 0.0, 0.0];
    store
        .upsert_tool_embedding_field(
            "weather",
            "summary",
            "s0",
            "hash1",
            "test-model",
            &emb,
            "weather tool",
        )
        .await
        .unwrap();

    let results = store.search_tools(&emb, 10, 0.0).await.unwrap();
    assert_eq!(results.len(), 1, "tool must be found via vec0");
    assert_eq!(results[0].0, "weather");

    store.delete_tool_embeddings("weather").await.unwrap();
    let results = store.search_tools(&emb, 10, 0.0).await.unwrap();
    assert_eq!(
        results.len(),
        0,
        "tool must be removed from vec0 after delete"
    );
}

/// The similarity threshold must filter results in the vec0 path.
#[tokio::test]
async fn vec0_search_respects_threshold() {
    let store = setup_store().await;

    let item = test_memory_item("threshold test", "content");
    let id = store.insert_typed_memory(&item).await.unwrap();
    // Orthogonal vector: cosine similarity to query [1,0,0,0] is 0.
    store
        .upsert_memory_embedding(id, "test-model", "content", &[0.0, 1.0, 0.0, 0.0])
        .await
        .unwrap();

    let query = vec![1.0, 0.0, 0.0, 0.0];

    let results = store
        .search_typed_memories_vector(&query, "ene", "test-model", None, &["active"], 10, 0.0)
        .await
        .unwrap();
    assert_eq!(
        results.len(),
        1,
        "orthogonal vector must pass threshold 0.0"
    );

    let results = store
        .search_typed_memories_vector(&query, "ene", "test-model", None, &["active"], 10, 0.5)
        .await
        .unwrap();
    assert_eq!(
        results.len(),
        0,
        "orthogonal vector must be filtered by threshold 0.5"
    );
}

/// Changing the embedding dimension must not brick the store: the vec0
/// shadow tables are dropped and recreated empty at the new dimension, the
/// old-dimension vectors are left out (they cannot fit the new `float[N]`
/// schema), and KNN search returns no rows instead of erroring.
#[tokio::test]
async fn vec0_dimension_change_rebuilds_empty_not_fail() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("memory.db");

    let store = MemoryStore::open(&path, 4).await.expect("open dim=4");
    let item = test_memory_item("dim change", "content before model switch");
    let id = store.insert_typed_memory(&item).await.unwrap();
    store
        .upsert_memory_embedding(id, "model-a", "content", &[1.0, 0.0, 0.0, 0.0])
        .await
        .unwrap();
    drop(store);

    // Reopen with a different embedding model (dim=8). This must succeed —
    // backfilling 4-dim vectors into a recreated float[8]
    // table would fail and leave `MemoryStore::open` erroring forever after.
    let store = MemoryStore::open(&path, 8).await.expect("reopen dim=8");
    let results = store
        .search_typed_memories_vector(
            &[1.0_f32, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
            "ene",
            "model-b",
            None,
            &["active"],
            10,
            0.0,
        )
        .await
        .unwrap();
    // Stale old-dimension vectors are not in the index until re-embedded.
    assert_eq!(
        results.len(),
        0,
        "old vectors must not appear after dim change"
    );

    let item2 = test_memory_item("dim change 2", "content after model switch");
    let id2 = store.insert_typed_memory(&item2).await.unwrap();
    store
        .upsert_memory_embedding(
            id2,
            "model-b",
            "content",
            &[0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
        )
        .await
        .unwrap();
    let results = store
        .search_typed_memories_vector(
            &[0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
            "ene",
            "model-b",
            None,
            &["active"],
            10,
            0.0,
        )
        .await
        .unwrap();
    assert_eq!(results.len(), 1, "new vectors must be searchable");
}

// ── audit_log session_id and session export tool history ──

/// Builds a [`crate::NewAuditEntry`] for test use.
fn test_audit_entry(session_id: &str, tool_name: &str) -> crate::NewAuditEntry {
    crate::NewAuditEntry {
        turn_id: format!("turn-{tool_name}"),
        session_id: Some(session_id.to_string()),
        tool_name: tool_name.to_string(),
        action: "write".to_string(),
        target: "/tmp/x".to_string(),
        decision: crate::AuditDecision::AllowOnce,
        success: true,
        arguments: r#"{"path":"/tmp/x"}"#.to_string(),
    }
}

/// Audit rows written within a session carry that session's id; rows
/// with an empty session id (out-of-band diagnostics) persist as `NULL`.
#[tokio::test]
async fn audit_rows_carry_session_id() {
    let store = setup_store().await;

    store
        .insert_audit_entry(&test_audit_entry("sess-a", "fs.write_file"))
        .await
        .expect("insert sess-a");
    store
        .insert_audit_entry(&crate::NewAuditEntry {
            session_id: None,
            ..test_audit_entry("", "diag.ping")
        })
        .await
        .expect("insert out-of-band");

    let by_session = store
        .list_audit_entries_by_session("sess-a")
        .await
        .expect("list by session");
    assert_eq!(by_session.len(), 1);
    let entry = by_session.first().expect("sess-a entry");
    assert_eq!(entry.session_id.as_deref(), Some("sess-a"));
    assert_eq!(entry.tool_name, "fs.write_file");

    let all = store.list_audit_entries(10).await.expect("list all");
    assert_eq!(all.len(), 2);
    let diag = all
        .iter()
        .find(|e| e.tool_name == "diag.ping")
        .expect("diag entry");
    assert_eq!(diag.session_id, None);
    assert!(
        store
            .list_audit_entries_by_session("")
            .await
            .expect("list empty session")
            .is_empty(),
        "NULL-session rows must not match an empty session filter"
    );
}

/// `build_export` returns the session's own tool history and never
/// another session's (or NULL-session) rows.
#[tokio::test]
async fn session_export_includes_only_own_tool_history() {
    let store = setup_store().await;
    store
        .upsert_session(&new_session_meta("sess-a", "card"))
        .await
        .expect("upsert sess-a");
    store
        .upsert_session(&new_session_meta("sess-b", "card"))
        .await
        .expect("upsert sess-b");

    store
        .insert_audit_entry(&test_audit_entry("sess-a", "fs.write_file"))
        .await
        .expect("insert sess-a tool");
    store
        .insert_audit_entry(&test_audit_entry("sess-b", "fs.read_file"))
        .await
        .expect("insert sess-b tool");
    store
        .insert_audit_entry(&crate::NewAuditEntry {
            session_id: None,
            ..test_audit_entry("", "diag.ping")
        })
        .await
        .expect("insert out-of-band tool");

    let export = store.build_export("sess-a").await.expect("export sess-a");
    assert_eq!(export.tool_logs.len(), 1);
    let log = export.tool_logs.first().expect("tool log");
    assert_eq!(log.tool_name, "fs.write_file");
    assert_eq!(log.decision, "allow_once");
    assert!(log.success);

    let export_b = store.build_export("sess-b").await.expect("export sess-b");
    assert_eq!(export_b.tool_logs.len(), 1);
    assert_eq!(
        export_b.tool_logs.first().expect("tool log").tool_name,
        "fs.read_file"
    );
}

/// A session with no audit rows still exports cleanly with an empty
/// `tool_logs` vector (existing NULL-session rows are not leaked in).
#[tokio::test]
async fn session_export_without_audit_rows_is_empty() {
    let store = setup_store().await;
    store
        .upsert_session(&new_session_meta("lonely", "card"))
        .await
        .expect("upsert lonely");
    store
        .insert_audit_entry(&crate::NewAuditEntry {
            session_id: None,
            ..test_audit_entry("", "diag.ping")
        })
        .await
        .expect("insert out-of-band tool");

    let export = store.build_export("lonely").await.expect("export");
    assert!(export.tool_logs.is_empty());
}

/// Bumping a memory's access counters (as recall does) must
/// not shield it from the forgetting lifecycle. If the decay anchor were
/// `last_accessed_at`, a recently-recalled memory would reset its age to zero
/// and never reach `FADE_THRESHOLD`. The anchor is `updated_at`, so a
/// stale-but-recalled memory still fades.
#[tokio::test]
async fn recall_bump_does_not_prevent_forgetting() {
    let store = setup_store().await;
    let now = Utc::now();

    let id = store
        .insert_typed_memory(&crate::NewMemoryItem {
            scope: crate::MemoryScope::Character,
            character_id: "ene".into(),
            user_id: "user1".into(),
            kind: crate::MemoryKind::Semantic,
            title: "recalled but stale".into(),
            content: "an old fact that keeps getting recalled".into(),
            source: crate::MemorySource::Conversation,
            source_ref: None,
            confidence: crate::MemoryConfidence::new(0.5),
            salience: crate::MemorySalience::new(0.5),
            affect: crate::AffectAnnotation::default(),
            relationship_impact: 0.0,
            valid_from: None,
            valid_until: None,
            status: crate::MemoryStatus::Active,
            supersedes_id: None,
            pinned: false,
            created_at: None,
            commitment_id: None,
        })
        .await
        .unwrap();
    store.test_backdate_typed_memory(id, 30).await.unwrap();

    // Simulate a recall that just happened: bump access counters repeatedly.
    for _ in 0..10 {
        assert!(store.bump_typed_memory_access(id).await.unwrap());
    }
    let recalled = store.get_typed_memory(id).await.unwrap().unwrap();
    assert_eq!(recalled.access_count, 10);
    assert!(recalled.last_accessed_at.is_some());

    let report = store
        .apply_natural_decay_batch(
            "ene",
            Some("user1"),
            now,
            30.0,
            ene_rag::FADE_THRESHOLD,
            ene_rag::ARCHIVE_THRESHOLD,
        )
        .await
        .unwrap();
    assert_eq!(
        report.faded_count, 1,
        "a recalled-but-stale memory must still fade (anchor = updated_at)"
    );
    let after = store.get_typed_memory(id).await.unwrap().unwrap();
    assert_eq!(after.status, crate::MemoryStatus::Faded);
}
