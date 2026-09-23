//! Bounded report queries and read-only separation: the Task/Action/History
//! report reads page in canonical order with SQL bounds, mutate nothing, and
//! start no runner, reconciliation, or lifecycle transition.

use super::*;

use ene_action::{ActionCertainty, EffectGrounds};
use ene_companion::UNDELIVERED_PAGE_MAX;
use ene_task::{
    REPORT_PAGE_MAX, TaskProgress, TaskReportRowCursor, TaskReportRowKind, TaskReportSourceRef,
};
use rusqlite::{OptionalExtension, params};

use super::task_result::{finalize, seed_workspace_execution, settle, start_attempt};

/// One fixed set of durable facts the read-only checks compare against.
#[derive(Debug, PartialEq, Eq)]
struct Snapshot {
    total_changes: u64,
    companions: i64,
    presence_generation: Option<i64>,
    task_revision: i64,
    task_progress: String,
    adopted_results: i64,
    undelivered: Vec<(String, String, String, String)>,
    delegations: i64,
    attempts: i64,
}

fn snapshot(store: &Store, companion: RawId, task: TaskId) -> Snapshot {
    let guard = match store.conn.lock() {
        Ok(locked) => locked,
        Err(poisoned) => poisoned.into_inner(),
    };
    let count = |sql: &str| -> i64 {
        guard
            .query_row(sql, (), |row| row.get(0))
            .expect("count reads")
    };
    let undelivered = {
        let mut statement = guard
            .prepare("SELECT source_kind, source_id, source_phase, status FROM undelivered ORDER BY row_seq")
            .expect("the undelivered query prepares");
        statement
            .query_map((), |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })
            .expect("the undelivered query runs")
            .collect::<rusqlite::Result<Vec<_>>>()
            .expect("the undelivered rows decode")
    };
    Snapshot {
        total_changes: guard.total_changes(),
        companions: count("SELECT COUNT(*) FROM companion"),
        presence_generation: {
            let found: Option<i64> = guard
                .query_row(
                    "SELECT generation FROM presence_attribution WHERE companion_id = ?1",
                    params![crate::codec::encode_id(companion)],
                    |row| row.get(0),
                )
                .optional()
                .expect("the generation probe runs");
            found
        },
        task_revision: guard
            .query_row(
                "SELECT revision FROM task WHERE task_id = ?1",
                params![crate::codec::encode_id(task.as_raw())],
                |row| row.get(0),
            )
            .expect("the revision reads"),
        task_progress: guard
            .query_row(
                "SELECT progress FROM task WHERE task_id = ?1",
                params![crate::codec::encode_id(task.as_raw())],
                |row| row.get(0),
            )
            .expect("the progress reads"),
        adopted_results: count(
            "SELECT COUNT(*) FROM task_result WHERE adopted_revision IS NOT NULL",
        ),
        undelivered,
        delegations: count("SELECT COUNT(*) FROM delegation"),
        attempts: count("SELECT COUNT(*) FROM action_attempt"),
    }
}

async fn run_report_reads(store: &Store, companion: CompanionId, task: TaskId) {
    let _ = store.list_tasks_after(None, REPORT_PAGE_MAX).await.unwrap();
    let _ = store
        .list_task_report_rows_after(task, None, REPORT_PAGE_MAX)
        .await
        .unwrap();
    let _ = store.list_unadopted_results_after(None, 10).await.unwrap();
    let _ = store
        .list_unpresented(companion, None, UNDELIVERED_PAGE_MAX)
        .await
        .unwrap();
    let _ = store
        .load_report_source_bounded(
            TaskReportSourceRef::RevisionPurpose {
                task,
                revision: TaskRevision::initial(),
            },
            0,
            64,
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn report_reads_are_read_only_over_a_running_and_stopped_store() {
    let store = open_memory().await.unwrap();
    let (companion, _) = running_companion(&store).await.unwrap();
    let (task, delegation, assoc) = seed_workspace_execution(&store).await;
    let attempt = start_attempt(&store, delegation, task, assoc, "report.txt").await;
    settle(
        &store,
        attempt,
        ActionCertainty::ConfirmedSuccess,
        EffectGrounds::ObservedAtTarget,
    )
    .await;
    // A sealed result that is deliberately left unadopted: a reconciliation
    // pass would evaluate it, so the read-only check can see the difference.
    let result = finalize(&store, delegation, "sealed, not adopted")
        .await
        .result;

    let before = snapshot(&store, companion.as_raw(), task.task);
    assert_eq!(before.task_progress, "in_progress");
    assert_eq!(
        before.adopted_results, 0,
        "the sealed result stays unadopted"
    );
    assert!(
        before.undelivered.len() >= 2,
        "producers registered entries"
    );

    for _ in 0..3 {
        run_report_reads(&store, companion, task.task).await;
        let headlines = store.list_tasks_after(None, REPORT_PAGE_MAX).await.unwrap();
        let headline = headlines
            .iter()
            .find(|headline| headline.task == task.task)
            .expect("the task is listed");
        assert_eq!(headline.revision, task.revision);
        assert_eq!(headline.progress, TaskProgress::InProgress);
        assert!(!headline.adopted_result);
        let rows = store
            .list_task_report_rows_after(task.task, None, REPORT_PAGE_MAX)
            .await
            .unwrap();
        assert!(
            rows.iter()
                .any(|row| row.kind == TaskReportRowKind::TaskResult && row.id == result.as_raw()),
            "the recorded result is a detail row"
        );
        assert!(
            rows.iter().all(|row| row.adopted_revision.is_none()),
            "nothing is adopted by a read"
        );
    }
    let after = snapshot(&store, companion.as_raw(), task.task);
    assert_eq!(
        after, before,
        "a running store keeps its presence generation, revision, statuses, and adoption"
    );

    // The same reads over a stopped companion return the saved facts and
    // mutate nothing; the reads are not lifecycle-gated.
    {
        let guard = match store.conn.lock() {
            Ok(locked) => locked,
            Err(poisoned) => poisoned.into_inner(),
        };
        let stopped = guard
            .execute(
                "UPDATE companion SET lifecycle = ?1 WHERE companion_id = ?2",
                params![
                    crate::codec::encode_lifecycle(CompanionLifecycle::Stopped),
                    crate::codec::encode_id(companion.as_raw())
                ],
            )
            .expect("the stop write applies");
        assert_eq!(stopped, 1);
    }
    let stopped_before = snapshot(&store, companion.as_raw(), task.task);
    for _ in 0..3 {
        run_report_reads(&store, companion, task.task).await;
    }
    let stopped_after = snapshot(&store, companion.as_raw(), task.task);
    assert_eq!(
        stopped_after, stopped_before,
        "a stopped store keeps its facts and starts nothing"
    );
    assert_eq!(
        stopped_after.presence_generation,
        before.presence_generation
    );
}

#[tokio::test]
async fn report_rows_order_attempts_before_results_and_page_by_keyset() {
    let store = open_memory().await.unwrap();
    let (task, delegation, assoc) = seed_workspace_execution(&store).await;
    let mut attempts = Vec::new();
    for name in ["a.txt", "b.txt", "c.txt"] {
        attempts.push(start_attempt(&store, delegation, task, assoc, name).await);
    }
    let result = finalize(&store, delegation, "final").await.result;

    let mut expected_attempts: Vec<String> = attempts
        .iter()
        .map(|attempt| crate::codec::encode_id(attempt.as_raw()))
        .collect();
    expected_attempts.sort();

    let rows = store
        .list_task_report_rows_after(task.task, None, REPORT_PAGE_MAX)
        .await
        .unwrap();
    assert_eq!(rows.len(), 4);
    let attempt_rows: Vec<&ene_task::TaskReportRow> = rows
        .iter()
        .filter(|row| row.kind == TaskReportRowKind::ActionAttempt)
        .collect();
    assert_eq!(
        attempt_rows
            .iter()
            .map(|row| crate::codec::encode_id(row.id))
            .collect::<Vec<_>>(),
        expected_attempts
    );
    assert_eq!(rows.last().unwrap().kind, TaskReportRowKind::TaskResult);
    assert_eq!(rows.last().unwrap().id, result.as_raw());
    assert_eq!(
        rows.last().unwrap().adopted_revision,
        None,
        "a recorded result is not adopted"
    );

    // A one-row page walks the same order through the cursor.
    let mut paged: Vec<(TaskReportRowKind, String)> = Vec::new();
    let mut after: Option<TaskReportRowCursor> = None;
    loop {
        let page = store
            .list_task_report_rows_after(task.task, after.clone(), 1)
            .await
            .unwrap();
        let Some(row) = page.first() else {
            break;
        };
        after = Some(TaskReportRowCursor {
            kind: row.kind,
            id: row.id,
        });
        paged.extend(
            page.iter()
                .map(|row| (row.kind, crate::codec::encode_id(row.id))),
        );
    }
    assert_eq!(paged.len(), 4);
    assert_eq!(paged.last().unwrap().0, TaskReportRowKind::TaskResult);
    let clamped_low = store
        .list_task_report_rows_after(task.task, None, 0)
        .await
        .unwrap();
    assert_eq!(clamped_low.len(), 1);
}

#[tokio::test]
async fn report_source_pages_are_byte_bounded_on_utf8_boundaries() {
    let store = open_memory().await.unwrap();
    let (task, delegation, _) = seed_workspace_execution(&store).await;
    let result = finalize(&store, delegation, "日本語").await.result;

    let first = store
        .load_report_source_bounded(TaskReportSourceRef::ResultBody(result), 0, 4)
        .await
        .unwrap()
        .expect("the result exists");
    assert_eq!(first.text, "日");
    assert_eq!(first.total_bytes, 9);
    assert_eq!(first.next, Some(3));

    let second = store
        .load_report_source_bounded(
            TaskReportSourceRef::ResultBody(result),
            first.next.unwrap(),
            4,
        )
        .await
        .unwrap()
        .expect("the result exists");
    assert_eq!(second.text, "本");
    assert_eq!(second.next, Some(6));

    let third = store
        .load_report_source_bounded(
            TaskReportSourceRef::ResultBody(result),
            second.next.unwrap(),
            4,
        )
        .await
        .unwrap()
        .expect("the result exists");
    assert_eq!(third.text, "語");
    assert_eq!(third.next, None, "the body is drained");

    // Past the end is an empty page with no next, and a missing row reports
    // absence instead of an empty success.
    let past_end = store
        .load_report_source_bounded(TaskReportSourceRef::ResultBody(result), 99, 16)
        .await
        .unwrap()
        .expect("the row still exists");
    assert_eq!(past_end.text, "");
    assert_eq!(past_end.next, None);
    let missing = store
        .load_report_source_bounded(
            TaskReportSourceRef::ResultBody(ene_task::TaskResultId::generate()),
            0,
            16,
        )
        .await
        .unwrap();
    assert_eq!(missing, None);

    // The revision purpose snapshot is read from the same byte-bounded path.
    let purpose = store
        .load_report_source_bounded(
            TaskReportSourceRef::RevisionPurpose {
                task: task.task,
                revision: task.revision,
            },
            0,
            11,
        )
        .await
        .unwrap()
        .expect("the revision snapshot exists");
    assert_eq!(purpose.text, "write the A");
    assert_eq!(purpose.next, Some(11));
}
