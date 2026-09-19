//! Undelivered source registration (AU1b): every Task/Action producer commits
//! its notification in the parent fact's transaction, the source key is
//! idempotent, reads never register, and the reporting status stays
//! non-sticky.

use super::*;

use ene_action::{
    ActionAttemptRepository as _, ActionCertainty, CertaintyUpdateOutcome, EffectGrounds,
};
use ene_companion::{
    PresentationMark, ReportStatus, ReportStatusTransition, TaskFact, TerminalKindWire,
    UNDELIVERED_PAGE_MAX, UndeliveredCursor, UndeliveredId, UndeliveredSource,
};
use ene_task::{
    TaskCancelOutcome, TaskCommitOutcome, TaskCommitPremise, TaskContextEntryId, TaskFailureKind,
    TaskFailureOutcome, TaskFailurePremise, TaskProgress, TaskResultAcceptance,
};

use super::task_result::{
    claim, finalize, raw_exec, seed_workspace_execution, settle, start_attempt,
};

/// One stored undelivered row as `(source_kind, source_id, source_phase,
/// status)`, in insertion order.
fn undelivered_rows(store: &Store, companion: RawId) -> Vec<(String, String, String, String)> {
    let guard = match store.conn.lock() {
        Ok(locked) => locked,
        Err(poisoned) => poisoned.into_inner(),
    };
    let mut statement = guard
        .prepare("SELECT source_kind, source_id, source_phase, status FROM undelivered WHERE companion_id = ?1 ORDER BY row_seq")
        .expect("the undelivered query prepares");
    statement
        .query_map(params![crate::codec::encode_id(companion)], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })
        .expect("the undelivered query runs")
        .collect::<rusqlite::Result<Vec<_>>>()
        .expect("the undelivered rows decode")
}

/// Counts one exact source key.
fn count_key(store: &Store, kind: &str, id: RawId, phase: &str) -> i64 {
    let guard = match store.conn.lock() {
        Ok(locked) => locked,
        Err(poisoned) => poisoned.into_inner(),
    };
    guard
        .query_row(
            "SELECT COUNT(*) FROM undelivered WHERE source_kind = ?1 AND source_id = ?2 AND source_phase = ?3",
            params![kind, crate::codec::encode_id(id), phase],
            |row| row.get(0),
        )
        .expect("the source-key count must read")
}

/// Counts every undelivered row for one companion.
fn count_companion(store: &Store, companion: RawId) -> i64 {
    let guard = match store.conn.lock() {
        Ok(locked) => locked,
        Err(poisoned) => poisoned.into_inner(),
    };
    guard
        .query_row(
            "SELECT COUNT(*) FROM undelivered WHERE companion_id = ?1",
            params![crate::codec::encode_id(companion)],
            |row| row.get(0),
        )
        .expect("the companion count must read")
}

fn task_text(task: TaskId) -> String {
    crate::codec::encode_id(task.as_raw())
}

async fn create_task_for_assignee(store: &Store, assignee: RawId) -> TaskRef {
    let mut premise = task_premise(None);
    premise.assignee = AssigneeRef {
        companion: assignee,
    };
    store
        .create_task(premise)
        .await
        .expect("the AU2 create must commit")
}

async fn steer(store: &Store, task: TaskRef) -> TaskCommitOutcome {
    store
        .forward_steering(TaskCommitPremise {
            expected: task,
            new_purpose: None,
            adopted_purpose_entry: TaskContextEntryId::generate(),
            adopted_instruction: None,
        })
        .await
        .expect("the AU4 steering must answer")
}

#[tokio::test]
async fn task_producers_register_their_source_key_once() {
    let store = open_memory().await.unwrap();
    let assignee = RawId::new();
    let task = create_task_for_assignee(&store, assignee).await;

    // AU2: the initial revision is a TaskRevision(T, 1) fact.
    assert_eq!(
        undelivered_rows(&store, assignee),
        vec![(
            String::from("task_revision"),
            task_text(task.task),
            String::from("1"),
            String::from("pending"),
        )]
    );

    // AU4: the forward registers the new revision as its own source key.
    let TaskCommitOutcome::CommittedAs(next) = steer(&store, task).await else {
        panic!("steering must commit");
    };
    assert_eq!(
        count_key(&store, "task_revision", task.task.as_raw(), "2"),
        1
    );
    assert_eq!(
        count_key(&store, "task_revision", task.task.as_raw(), "1"),
        1,
        "the previous revision keeps its own row"
    );

    // AU3: the delegation registers for the delegator (the assignee).
    let delegation = DelegationId::generate();
    let outcome = store
        .create_delegation(delegation_premise(
            delegation,
            next,
            TaskAgentEphemeralId::generate(),
            delegation_scope(None),
        ))
        .await
        .unwrap();
    assert!(
        matches!(outcome, DelegationOutcome::Delegated(_)),
        "the delegation must commit"
    );
    assert_eq!(count_key(&store, "delegation", delegation.as_raw(), ""), 1);

    // AU16: the cancel admission registers the terminal fact once; a repeated
    // cancel writes nothing (and therefore registers nothing).
    assert_eq!(
        store.cancel_task(task.task).await.unwrap(),
        TaskCancelOutcome::CancelAccepted
    );
    assert_eq!(
        count_key(&store, "terminal", task.task.as_raw(), "cancelled"),
        1
    );
    assert_eq!(
        store.cancel_task(task.task).await.unwrap(),
        TaskCancelOutcome::AlreadyCancelled
    );
    assert_eq!(
        count_key(&store, "terminal", task.task.as_raw(), "cancelled"),
        1,
        "an idempotent repeat must not duplicate the notification"
    );
}

#[tokio::test]
async fn confirmed_failure_registers_the_terminal_fact_once() {
    let store = open_memory().await.unwrap();
    let assignee = RawId::new();
    let task = create_task_for_assignee(&store, assignee).await;
    let premise = TaskFailurePremise {
        task,
        delegation: None,
        kind: TaskFailureKind::ConfirmedUnachievable,
    };
    assert!(matches!(
        store.fail_task(premise).await.unwrap(),
        TaskFailureOutcome::FailedAs(_)
    ));
    assert_eq!(
        count_key(&store, "terminal", task.task.as_raw(), "failed"),
        1
    );
    assert!(matches!(
        store.fail_task(premise).await.unwrap(),
        TaskFailureOutcome::AlreadyFailed { .. }
    ));
    assert_eq!(
        count_key(&store, "terminal", task.task.as_raw(), "failed"),
        1
    );
    assert_eq!(
        count_companion(&store, assignee),
        2,
        "initial revision plus the terminal fact"
    );
}

#[tokio::test]
async fn action_and_result_producers_register_under_the_task_assignee() {
    let store = open_memory().await.unwrap();
    let (task, delegation, assoc) = seed_workspace_execution(&store).await;
    let assignee = store
        .load_task(task.task)
        .await
        .unwrap()
        .unwrap()
        .revision
        .assignee
        .companion;

    let first = start_attempt(&store, delegation, task, assoc, "first.txt").await;
    assert_eq!(
        count_key(&store, "action_attempt", first.as_raw(), "unknown"),
        1,
        "the AU5 start registers the attempt as unknown"
    );

    // The certainty CAS is a new source key; the old unknown row stays.
    settle(
        &store,
        first,
        ActionCertainty::ConfirmedSuccess,
        EffectGrounds::ObservedAtTarget,
    )
    .await;
    assert_eq!(
        count_key(&store, "action_attempt", first.as_raw(), "unknown"),
        1
    );
    assert_eq!(
        count_key(
            &store,
            "action_attempt",
            first.as_raw(),
            "confirmed_success"
        ),
        1
    );

    // A grounded unknown -> unknown update reuses the start phase: the
    // source-key constraint keeps it a no-op.
    let second = start_attempt(&store, delegation, task, assoc, "second.txt").await;
    let unverified = store
        .compare_and_set_certainty(
            second,
            ActionCertainty::Unknown,
            ActionCertainty::Unknown,
            EffectGrounds::OutcomeUnverified,
        )
        .await
        .unwrap();
    assert_eq!(unverified, CertaintyUpdateOutcome::Updated);
    assert_eq!(
        count_key(&store, "action_attempt", second.as_raw(), "unknown"),
        1
    );

    // AU15a registers the recorded result.
    let result = finalize(&store, delegation, "done").await.result;
    assert_eq!(count_key(&store, "result_recorded", result.as_raw(), ""), 1);

    // The still-unknown second attempt withholds adoption: no adoption fact
    // is registered and the lifecycle is unchanged.
    let withheld = store
        .adopt_result(claim(result, &[first, second]))
        .await
        .unwrap();
    assert!(
        matches!(withheld, TaskResultAcceptance::WithheldByEffectFacts { .. }),
        "the unknown attempt must withhold completion"
    );
    assert_eq!(count_key(&store, "result_adopted", result.as_raw(), ""), 0);

    settle(
        &store,
        second,
        ActionCertainty::ConfirmedSuccess,
        EffectGrounds::ObservedAtTarget,
    )
    .await;
    let adopted = store
        .adopt_result(claim(result, &[first, second]))
        .await
        .unwrap();
    assert!(matches!(
        adopted,
        TaskResultAcceptance::AdoptedAsCompletion(_)
    ));
    assert_eq!(count_key(&store, "result_adopted", result.as_raw(), ""), 1);

    // The idempotent adoption retry commits nothing new.
    let retried = store
        .adopt_result(claim(result, &[first, second]))
        .await
        .unwrap();
    assert!(matches!(
        retried,
        TaskResultAcceptance::AdoptedAsCompletion(_)
    ));
    assert_eq!(count_key(&store, "result_adopted", result.as_raw(), ""), 1);
    assert_eq!(
        count_companion(&store, assignee),
        8,
        "revision, delegation, two unknown phases, two confirmations, recorded, adopted"
    );
}

#[tokio::test]
async fn recorded_to_original_only_registers_no_adoption_fact() {
    let store = open_memory().await.unwrap();
    let (task, delegation, assoc) = seed_workspace_execution(&store).await;
    let attempt = start_attempt(&store, delegation, task, assoc, "late.txt").await;
    settle(
        &store,
        attempt,
        ActionCertainty::ConfirmedSuccess,
        EffectGrounds::ObservedAtTarget,
    )
    .await;
    let result = finalize(&store, delegation, "late result").await.result;

    assert_eq!(
        store.cancel_task(task.task).await.unwrap(),
        TaskCancelOutcome::CancelAccepted
    );
    let recorded_only = store.adopt_result(claim(result, &[attempt])).await.unwrap();
    assert_eq!(
        recorded_only,
        TaskResultAcceptance::RecordedToOriginalOnly,
        "a cancelled Task keeps the result in its original record only"
    );
    // RecordedToOriginalOnly commits correlation only: it is not an adoption,
    // so no ResultAdopted notification exists. The terminal Cancelled fact
    // already describes the lifecycle, and a fabricated adoption would claim
    // a completion that never happened.
    assert_eq!(count_key(&store, "result_adopted", result.as_raw(), ""), 0);
    assert_eq!(count_key(&store, "result_recorded", result.as_raw(), ""), 1);
}

#[tokio::test]
async fn a_failed_registration_rolls_the_parent_fact_back() {
    let store = open_memory().await.unwrap();
    let assignee = RawId::new();
    // A trigger that refuses every undelivered insert stands in for any
    // registration failure: the parent commit must roll back with it.
    raw_exec(
        &store,
        "CREATE TRIGGER test_refuse_undelivered BEFORE INSERT ON undelivered BEGIN SELECT RAISE(ABORT, 'test refusal'); END;",
    );
    let mut premise = task_premise(None);
    premise.assignee = AssigneeRef {
        companion: assignee,
    };
    let created = store.create_task(premise).await;
    assert!(
        created.is_err(),
        "a failed registration must fail the commit"
    );
    assert_eq!(task_table_count(&store, "task"), 0, "no parent Task row");
    assert_eq!(task_table_count(&store, "task_revision"), 0);
    assert_eq!(task_table_count(&store, "task_context_entry"), 0);

    // The cancel path shares the same atom: a refused registration leaves the
    // Task non-terminal instead of accepting the cancel.
    raw_exec(&store, "DROP TRIGGER test_refuse_undelivered");
    let task = create_task_for_assignee(&store, assignee).await;
    raw_exec(
        &store,
        "CREATE TRIGGER test_refuse_undelivered BEFORE INSERT ON undelivered BEGIN SELECT RAISE(ABORT, 'test refusal'); END;",
    );
    assert!(store.cancel_task(task.task).await.is_err());
    assert_eq!(
        store
            .load_task(task.task)
            .await
            .unwrap()
            .unwrap()
            .task
            .progress,
        TaskProgress::Started,
        "the cancel admission must not be durable without its notification"
    );
}

#[tokio::test]
async fn bounded_listing_pages_without_gaps_and_defers_new_rows_to_the_next_pass() {
    let store = open_memory().await.unwrap();
    let (companion, generation) = running_companion(&store).await.unwrap();
    for index in 0..52 {
        let (outcome, registered) = store
            .append_reply_with_undelivered(
                history_command(companion, generation, &format!("reply {index}")),
                true,
                None,
            )
            .await
            .unwrap();
        assert!(matches!(outcome, HistoryAppendOutcome::CommittedAs { .. }));
        assert!(registered.is_some());
    }

    // The first pass reads 50, the second the remaining 2, with no gap and no
    // overlap.
    let first = store
        .list_unpresented(companion, None, UNDELIVERED_PAGE_MAX)
        .await
        .unwrap();
    assert_eq!(first.entries.len(), 50);
    assert_eq!(first.pass_upper_bound, 52);
    let mut seen: std::collections::HashSet<RawId> = first
        .entries
        .iter()
        .map(|entry| entry.id.as_raw())
        .collect();
    let second = store
        .list_unpresented(companion, first.next, UNDELIVERED_PAGE_MAX)
        .await
        .unwrap();
    assert_eq!(second.entries.len(), 2);
    assert_eq!(second.next, None, "the pass is drained");
    seen.extend(second.entries.iter().map(|entry| entry.id.as_raw()));
    assert_eq!(seen.len(), 52, "paging must yield every entry exactly once");

    // A pass holds its captured binder: rows registered while it runs are
    // returned by the next pass, not skipped and not duplicated.
    let bound = store.undelivered_pass_bound().await.unwrap();
    for index in 52..55 {
        let (outcome, _) = store
            .append_reply_with_undelivered(
                history_command(companion, generation, &format!("late {index}")),
                true,
                None,
            )
            .await
            .unwrap();
        assert!(matches!(outcome, HistoryAppendOutcome::CommittedAs { .. }));
    }
    let pass = UndeliveredCursor::begin(0, bound);
    let page_one = store
        .list_unpresented(companion, Some(pass), 50)
        .await
        .unwrap();
    assert_eq!(page_one.entries.len(), 50);
    let page_two = store
        .list_unpresented(companion, page_one.next, 50)
        .await
        .unwrap();
    assert_eq!(
        page_two.entries.len(),
        2,
        "the pass stops at its captured upper bound"
    );
    assert_eq!(page_two.next, None);

    // The next pass starts above the already-scanned sequence and yields
    // exactly the late rows.
    let next_bound = store.undelivered_pass_bound().await.unwrap();
    assert_eq!(next_bound, 55);
    let resumed = store
        .list_unpresented(
            companion,
            Some(UndeliveredCursor::begin(bound, next_bound)),
            50,
        )
        .await
        .unwrap();
    assert_eq!(resumed.entries.len(), 3);
    assert_eq!(resumed.next, None);

    // The page bound is clamped to 1..=50 by the storage query.
    let clamped_low = store
        .list_unpresented(companion, Some(UndeliveredCursor::begin(0, next_bound)), 0)
        .await
        .unwrap();
    assert_eq!(clamped_low.entries.len(), 1);
    let clamped_high = store
        .list_unpresented(
            companion,
            Some(UndeliveredCursor::begin(0, next_bound)),
            1000,
        )
        .await
        .unwrap();
    assert_eq!(clamped_high.entries.len(), 50);
}

#[tokio::test]
async fn reads_register_nothing_and_excerpts_stay_bounded() {
    let store = open_memory().await.unwrap();
    let (companion, generation) = running_companion(&store).await.unwrap();
    let body = "日本語の本文です";
    let (_, registered) = store
        .append_reply_with_undelivered(history_command(companion, generation, body), true, None)
        .await
        .unwrap();
    let entry = registered.unwrap();
    let source = entry.source;
    let before = count_companion(&store, companion.as_raw());
    assert_eq!(before, 1);

    // Repeated reads of every undelivered read path register nothing.
    for _ in 0..3 {
        let _ = store
            .list_unpresented(companion, None, UNDELIVERED_PAGE_MAX)
            .await
            .unwrap();
        let _ = store.undelivered_pass_bound().await.unwrap();
        let _ = store.load_undelivered_excerpt(source, 4).await.unwrap();
    }
    assert_eq!(count_companion(&store, companion.as_raw()), before);

    // The excerpt is byte-bounded and cut on a character boundary, and
    // `total_bytes` keeps the full length.
    let excerpt = store
        .load_undelivered_excerpt(source, 4)
        .await
        .unwrap()
        .expect("the history source exists");
    assert_eq!(excerpt.text, "日");
    assert_eq!(excerpt.total_bytes, body.len() as u64);
    let full = store
        .load_undelivered_excerpt(source, 1024)
        .await
        .unwrap()
        .expect("the history source exists");
    assert_eq!(full.text, body);

    // A missing row reports absence, and a bodyless source kind has no
    // excerpt instead of an empty success.
    let missing = store
        .load_undelivered_excerpt(UndeliveredSource::HistoryMessage(RawId::new()), 16)
        .await
        .unwrap();
    assert_eq!(missing, None);
    let bodyless = store
        .load_undelivered_excerpt(
            UndeliveredSource::TaskRecord {
                task: RawId::new(),
                fact: TaskFact::Terminal {
                    task: RawId::new(),
                    progress: TerminalKindWire::Cancelled,
                },
            },
            16,
        )
        .await
        .unwrap();
    assert_eq!(bodyless, None);

    // Task sources read their owner rows: the revision purpose snapshot.
    let (task, _, _) = seed_workspace_execution(&store).await;
    let revision_source = UndeliveredSource::TaskRecord {
        task: task.task.as_raw(),
        fact: TaskFact::TaskRevision {
            task: task.task.as_raw(),
            revision: task.revision.as_u64(),
        },
    };
    let purpose = store
        .load_undelivered_excerpt(revision_source, 1024)
        .await
        .unwrap()
        .expect("the revision snapshot exists");
    assert_eq!(purpose.text, "write the AU2 slice");
}

#[tokio::test]
async fn undelivered_unknown_is_relisted_and_receipts_never_downgrade_presented() {
    let store = open_memory().await.unwrap();
    let (companion, generation) = running_companion(&store).await.unwrap();
    let (_, registered) = store
        .append_reply_with_undelivered(history_command(companion, generation, "body"), true, None)
        .await
        .unwrap();
    let entry = registered.unwrap();
    let round = entry.round.unwrap();

    // Presentation start marks Unknown, and Unknown is listed again.
    assert_eq!(
        store
            .compare_and_mark_reported(
                entry.id,
                ReportStatus::Pending,
                PresentationMark {
                    round,
                    presented: false,
                },
            )
            .await,
        Ok(ReportStatusTransition::MarkedPresentationUnknown)
    );
    let listed = store
        .list_unpresented(companion, None, UNDELIVERED_PAGE_MAX)
        .await
        .unwrap();
    assert_eq!(listed.entries.len(), 1);
    assert_eq!(listed.entries[0].status, ReportStatus::PresentationUnknown);

    // A mismatched expected status writes nothing.
    assert_eq!(
        store
            .compare_and_mark_reported(
                entry.id,
                ReportStatus::Pending,
                PresentationMark {
                    round,
                    presented: true,
                },
            )
            .await,
        Ok(ReportStatusTransition::StaleSource)
    );

    // A later confirmed presentation absorbs the row, then neither a
    // duplicate ACK nor a stale not-presented receipt moves it.
    assert_eq!(
        store
            .compare_and_mark_reported(
                entry.id,
                ReportStatus::PresentationUnknown,
                PresentationMark {
                    round,
                    presented: true,
                },
            )
            .await,
        Ok(ReportStatusTransition::PendingToPresented)
    );
    assert_eq!(
        store
            .compare_and_mark_reported(
                entry.id,
                ReportStatus::Presented,
                PresentationMark {
                    round,
                    presented: true,
                },
            )
            .await,
        Ok(ReportStatusTransition::AlreadyPresented)
    );
    assert_eq!(
        store
            .compare_and_mark_reported(
                entry.id,
                ReportStatus::PresentationUnknown,
                PresentationMark {
                    round,
                    presented: false,
                },
            )
            .await,
        Ok(ReportStatusTransition::AlreadyPresented),
        "presented absorbs a later not-presented receipt"
    );
    assert_eq!(
        store
            .compare_and_mark_reported(
                entry.id,
                ReportStatus::Pending,
                PresentationMark {
                    round,
                    presented: true,
                },
            )
            .await,
        Ok(ReportStatusTransition::AlreadyPresented),
        "a presented row absorbs every later mark, stale premise included"
    );
}

/// The exact-identity read resolves requested ids in their requested order
/// with stored statuses, and omits unknown or foreign identities so a
/// receipt can detect an unrehydratable selection.
#[tokio::test]
async fn exact_identity_reads_preserve_order_and_resolve_statuses() {
    let store = open_memory().await.unwrap();
    let assignee = RawId::new();
    let task = create_task_for_assignee(&store, assignee).await;
    let TaskCommitOutcome::CommittedAs(_) = steer(&store, task).await else {
        panic!("steering must commit");
    };
    let companion = CompanionId::from_raw(assignee);
    let page = store
        .list_unpresented(companion, None, 50)
        .await
        .expect("the listing must read");
    assert_eq!(page.entries.len(), 2, "revision 1 and revision 2");
    let first = page.entries[0].id;
    let second = page.entries[1].id;

    // Requested order wins, never insertion order.
    let loaded = store
        .load_undelivered_by_ids(companion, &[second, first])
        .await
        .expect("the exact read must answer");
    assert_eq!(
        loaded.iter().map(|entry| entry.id).collect::<Vec<_>>(),
        vec![second, first]
    );

    // A presented row still resolves, with its stored status; an unknown
    // identity is omitted, so the caller sees the shorter vector and can
    // fail closed.
    let transition = store
        .compare_and_mark_reported(
            second,
            ReportStatus::Pending,
            PresentationMark {
                round: RawId::new(),
                presented: true,
            },
        )
        .await
        .expect("the presentation compare must answer");
    assert_eq!(transition, ReportStatusTransition::PendingToPresented);
    let unknown = UndeliveredId::from_raw(RawId::new());
    let loaded = store
        .load_undelivered_by_ids(companion, &[first, second, unknown])
        .await
        .expect("the exact read must answer");
    assert_eq!(
        loaded.iter().map(|entry| entry.id).collect::<Vec<_>>(),
        vec![first, second],
        "presented and pending identities both resolve"
    );
    assert_eq!(loaded[0].status, ReportStatus::Pending);
    assert_eq!(loaded[1].status, ReportStatus::Presented);

    // A foreign companion never resolves another companion's identity, even
    // with the exact id.
    let other = store
        .load_undelivered_by_ids(CompanionId::from_raw(RawId::new()), &[first])
        .await
        .expect("the exact read must answer");
    assert!(other.is_empty());

    // The read changes nothing.
    let after = store
        .list_unpresented(companion, None, 50)
        .await
        .expect("the listing must read");
    assert_eq!(after.entries.len(), 1);
    assert_eq!(after.entries[0].id, first);
}
