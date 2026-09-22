use std::collections::HashMap;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use ene_preservation::{
    DemandLocalErasureCommand, ErasureConditionRef, ErasureParticipant, MechanicalDeletionTarget,
    ParticipantCompletionFact, ParticipantHoldClass, ParticipantOwnerRef,
};
use ene_primitive::WallClockWithTz;
use rusqlite::{Connection, TransactionBehavior, params};

use crate::Store;
use crate::codec::{encode_id, lock_shared};
use crate::run_blocking;

use super::redact_exact;

pub(crate) const ROWS_PER_DEMAND: u32 = 64;

const PAGE_ROWS: u32 = 32;

#[cfg(windows)]
pub(crate) const ERASED_LOCATOR: &str = r"C:\erased";
#[cfg(not(windows))]
pub(crate) const ERASED_LOCATOR: &str = "/erased";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ErasureShape {
    Text,
    AbsolutePath,
}

#[derive(Debug, Clone, Copy)]
struct ErasureColumn {
    name: &'static str,
    shape: ErasureShape,
}

#[derive(Debug, Clone, Copy)]
struct ErasureStage {
    table: &'static str,
    columns: &'static [ErasureColumn],
}

const PURPOSE_TEXT: ErasureColumn = ErasureColumn {
    name: "purpose_text",
    shape: ErasureShape::Text,
};

const TASK_BODY: ErasureColumn = ErasureColumn {
    name: "body",
    shape: ErasureShape::Text,
};

const OBSERVATION_PATH: ErasureColumn = ErasureColumn {
    name: "path",
    shape: ErasureShape::AbsolutePath,
};

const WORKSPACE_FOLDER: ErasureColumn = ErasureColumn {
    name: "folder",
    shape: ErasureShape::Text,
};

const WORKSPACE_SAVE_TARGET: ErasureColumn = ErasureColumn {
    name: "save_target",
    shape: ErasureShape::Text,
};

const DELEGATION_SCOPE_FOLDER: ErasureColumn = ErasureColumn {
    name: "scope_folder",
    shape: ErasureShape::Text,
};

const DELEGATION_SCOPE_SAVE_TARGET: ErasureColumn = ErasureColumn {
    name: "scope_save_target",
    shape: ErasureShape::Text,
};

const ACTION_REAL_TARGET: ErasureColumn = ErasureColumn {
    name: "real_target",
    shape: ErasureShape::AbsolutePath,
};

const TASK_STAGES: &[ErasureStage] = &[
    ErasureStage {
        table: "task",
        columns: &[PURPOSE_TEXT],
    },
    ErasureStage {
        table: "task_revision",
        columns: &[PURPOSE_TEXT],
    },
    ErasureStage {
        table: "task_result",
        columns: &[TASK_BODY],
    },
    ErasureStage {
        table: "task_agent_observation",
        columns: &[OBSERVATION_PATH],
    },
    ErasureStage {
        table: "workspace_assoc",
        columns: &[WORKSPACE_FOLDER, WORKSPACE_SAVE_TARGET],
    },
    ErasureStage {
        table: "delegation",
        columns: &[DELEGATION_SCOPE_FOLDER, DELEGATION_SCOPE_SAVE_TARGET],
    },
];

const ACTION_STAGES: &[ErasureStage] = &[ErasureStage {
    table: "action_attempt",
    columns: &[ACTION_REAL_TARGET],
}];

const INFERENCE_STAGES: &[ErasureStage] = &[];

struct ValueRedaction {
    column: &'static str,
    value: String,
    removed: u64,
}

#[derive(Debug, Clone, Copy)]
struct SweepProgress {
    phase: SweepPhase,
    stage: usize,
    after_rowid: i64,
    erased: u64,
    verify_found: u64,
}

impl Default for SweepProgress {
    fn default() -> Self {
        Self {
            phase: SweepPhase::Erase,
            stage: 0,
            after_rowid: 0,
            erased: 0,
            verify_found: 0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SweepPhase {
    Erase,
    Verify,
}

enum Chunk {
    EraseComplete { erased: u64 },
    MoreWork { erased: u64, remainder: u64 },
    Verified { erased: u64 },
}

struct ErasePage {
    last_rowid: i64,
    examined: u32,
    redacted: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ErasurePageError {
    Storage,
    Unrepresentable,
    NotCurrent,
}

type DeletionSweepKey = (
    ene_preservation::DeletionOperationId,
    ene_preservation::DeletionSweepGeneration,
);

struct ErasureCore {
    owner: ParticipantOwnerRef,
    stages: &'static [ErasureStage],
    store: Store,
    sweeps: tokio::sync::Mutex<HashMap<DeletionSweepKey, SweepProgress>>,
}

impl ErasureCore {
    fn new(owner: ParticipantOwnerRef, stages: &'static [ErasureStage], store: Store) -> Self {
        Self {
            owner,
            stages,
            store,
            sweeps: tokio::sync::Mutex::new(HashMap::new()),
        }
    }

    async fn demand(&self, command: DemandLocalErasureCommand) -> ParticipantCompletionFact {
        #[cfg(any(test, feature = "test-support"))]
        self.store
            .test_parks
            .erasure_mutation
            .pause_if_armed()
            .await;
        let condition = command.condition();
        let observed_at = WallClockWithTz::now();
        let Some(target) = exact_target(&command) else {
            return ParticipantCompletionFact::held(
                condition,
                self.owner,
                ParticipantHoldClass::Failed,
                observed_at,
            );
        };
        let key = (condition.operation, condition.sweep);
        let mut sweeps = self.sweeps.lock().await;
        let progress = sweeps.entry(key).or_default();
        match self.advance(progress, &target, condition).await {
            Ok(Chunk::EraseComplete { erased }) => ParticipantCompletionFact::local_complete(
                condition,
                self.owner,
                erased,
                0,
                observed_at,
            ),
            Ok(Chunk::MoreWork { erased, remainder }) => ParticipantCompletionFact::more_work(
                condition,
                self.owner,
                erased,
                remainder,
                observed_at,
            ),
            Ok(Chunk::Verified { erased }) => {
                sweeps.remove(&key);
                ParticipantCompletionFact::verified(condition, self.owner, erased, observed_at)
            }
            Err(ErasurePageError::Storage) => ParticipantCompletionFact::held(
                condition,
                self.owner,
                ParticipantHoldClass::Unavailable,
                observed_at,
            ),
            Err(ErasurePageError::Unrepresentable) => ParticipantCompletionFact::held(
                condition,
                self.owner,
                ParticipantHoldClass::Failed,
                observed_at,
            ),
            // Not-current work mutates nothing and cannot verify the demanded
            // condition; the durable record refuses it as stale or completed.
            // A closed generation is terminal and can never become current
            // again, so its cursor is dead state rather than unfinished work:
            // drop it instead of leaking one entry per raced demand.
            Err(ErasurePageError::NotCurrent) => {
                sweeps.remove(&key);
                ParticipantCompletionFact::local_complete(condition, self.owner, 0, 0, observed_at)
            }
        }
    }

    async fn advance(
        &self,
        progress: &mut SweepProgress,
        target: &str,
        condition: ErasureConditionRef,
    ) -> Result<Chunk, ErasurePageError> {
        if self.stages.is_empty() {
            let conn = Arc::clone(&self.store.conn);
            let current = run_blocking(move || {
                let guard = lock_shared(&conn);
                crate::preservation::condition_is_current(&guard, condition)
                    .map_err(|_| ErasurePageError::Storage)
            })
            .await?;
            if !current {
                return Err(ErasurePageError::NotCurrent);
            }
        }
        let mut examined = 0u32;
        loop {
            if progress.stage >= self.stages.len() {
                match progress.phase {
                    SweepPhase::Erase => {
                        progress.phase = SweepPhase::Verify;
                        progress.stage = 0;
                        progress.after_rowid = 0;
                        progress.verify_found = 0;
                        return Ok(Chunk::EraseComplete {
                            erased: progress.erased,
                        });
                    }
                    SweepPhase::Verify if progress.verify_found == 0 => {
                        return Ok(Chunk::Verified {
                            erased: progress.erased,
                        });
                    }
                    SweepPhase::Verify => {
                        let remainder = progress.verify_found;
                        progress.stage = 0;
                        progress.after_rowid = 0;
                        progress.verify_found = 0;
                        return Ok(Chunk::MoreWork {
                            erased: progress.erased,
                            remainder,
                        });
                    }
                }
            }
            if examined >= ROWS_PER_DEMAND {
                return Ok(Chunk::MoreWork {
                    erased: progress.erased,
                    remainder: progress.verify_found,
                });
            }
            let stage = &self.stages[progress.stage];
            let limit = PAGE_ROWS.min(ROWS_PER_DEMAND - examined);
            let page = erase_page(
                &self.store,
                stage,
                target,
                condition,
                progress.after_rowid,
                limit,
            )
            .await?;
            if page.examined == 0 {
                progress.stage += 1;
                progress.after_rowid = 0;
                continue;
            }
            examined += page.examined;
            progress.after_rowid = page.last_rowid;
            progress.erased += page.redacted;
            if progress.phase == SweepPhase::Verify {
                progress.verify_found += page.redacted;
            }
        }
    }
}

fn exact_target(command: &DemandLocalErasureCommand) -> Option<String> {
    let target = command.scope().target()?;
    let MechanicalDeletionTarget::ExactText(material) = &target.mechanical;
    let exact = material.expose_for_erasure();
    if exact.trim().is_empty() {
        return None;
    }
    Some(exact.to_owned())
}

async fn erase_page(
    store: &Store,
    stage: &'static ErasureStage,
    target: &str,
    condition: ErasureConditionRef,
    after_rowid: i64,
    limit: u32,
) -> Result<ErasePage, ErasurePageError> {
    let conn = Arc::clone(&store.conn);
    let target = target.to_owned();
    run_blocking(move || erase_page_sync(&conn, stage, &target, condition, after_rowid, limit))
        .await
}

fn erase_page_sync(
    conn: &Mutex<Connection>,
    stage: &ErasureStage,
    target: &str,
    condition: ErasureConditionRef,
    after_rowid: i64,
    limit: u32,
) -> Result<ErasePage, ErasurePageError> {
    let mut guard = lock_shared(conn);
    let tx = guard
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|_| ErasurePageError::Storage)?;
    if !crate::preservation::condition_is_current(&tx, condition)
        .map_err(|_| ErasurePageError::Storage)?
    {
        return Err(ErasurePageError::NotCurrent);
    }
    let operation = encode_id(condition.operation.as_raw());
    let rows = {
        let mut statement = tx
            .prepare(&page_sql(stage))
            .map_err(|_| ErasurePageError::Storage)?;
        statement
            .query_map(params![after_rowid, limit], |row| {
                let mut values = Vec::with_capacity(stage.columns.len());
                for index in 0..stage.columns.len() {
                    values.push(row.get::<_, Option<String>>(index + 1)?);
                }
                Ok(RawErasureRow {
                    rowid: row.get(0)?,
                    values,
                })
            })
            .map_err(|_| ErasurePageError::Storage)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| ErasurePageError::Storage)?
    };
    let mut examined = 0u32;
    let mut redacted = 0u64;
    let mut last_rowid = after_rowid;
    for row in &rows {
        examined += 1;
        last_rowid = row.rowid;
        let provenance_linked = stage.table == "task_result"
            && result_delegation_held_for_operation(&tx, row.rowid, &operation)?;
        for value in plan_redactions(stage, row, target, provenance_linked)? {
            let sql = format!(
                "UPDATE {} SET {} = ?1 WHERE rowid = ?2",
                stage.table, value.column
            );
            tx.execute(&sql, params![value.value, row.rowid])
                .map_err(|_| ErasurePageError::Storage)?;
            redacted += value.removed;
        }
    }
    tx.commit().map_err(|_| ErasurePageError::Storage)?;
    Ok(ErasePage {
        last_rowid,
        examined,
        redacted,
    })
}

fn result_delegation_held_for_operation(
    tx: &rusqlite::Transaction<'_>,
    rowid: i64,
    operation: &str,
) -> Result<bool, ErasurePageError> {
    tx.query_row(
        "SELECT EXISTS(
             SELECT 1 FROM task_result r
             JOIN erasure_use_hold h
               ON h.use_kind = 'task_delegation' AND h.use_id = r.delegation_id
             WHERE r.rowid = ?1 AND h.operation_id = ?2
         )",
        params![rowid, operation],
        |row| row.get(0),
    )
    .map_err(|_| ErasurePageError::Storage)
}

struct RawErasureRow {
    rowid: i64,
    values: Vec<Option<String>>,
}

fn page_sql(stage: &ErasureStage) -> String {
    let columns = stage
        .columns
        .iter()
        .map(|column| column.name)
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "SELECT rowid, {columns} FROM {} WHERE rowid > ?1 ORDER BY rowid LIMIT ?2",
        stage.table
    )
}

/// Plans the redactions of one row. Nothing is written until the whole page
/// has been read and planned, so an unrepresentable value fails the page
/// closed instead of leaving it half-swept.
///
/// `provenance_linked` is the observation-hold path for `task_result`: the
/// whole body is replaced by the body-free marker because a paraphrase of a
/// discarded observation cannot be proven unrelated to the target by
/// mechanical search. An already-collected marker is left untouched only when
/// the marker itself is target-free; a target that overlaps the marker is
/// redacted out of it so the verify pass stays a clean pass. The execution
/// seal itself is the row's existence and is never rewritten.
fn plan_redactions(
    stage: &ErasureStage,
    row: &RawErasureRow,
    target: &str,
    provenance_linked: bool,
) -> Result<Vec<ValueRedaction>, ErasurePageError> {
    let mut planned = Vec::new();
    for (column, value) in stage.columns.iter().zip(&row.values) {
        let Some(text) = value.as_deref() else {
            continue;
        };
        if provenance_linked && column.name == "body" {
            // The fixed marker is target-free only when the target is not a
            // substring of it. A target that overlaps the marker must go
            // through the same mechanical predicate as every other value
            // instead of being certified clean by identity.
            let replacement = erase_exact(super::ERASED_MARKER, target).map_or_else(
                || String::from(super::ERASED_MARKER),
                |(redacted, _)| redacted,
            );
            if replacement.as_str() != text {
                planned.push(ValueRedaction {
                    column: column.name,
                    value: replacement,
                    removed: 1,
                });
            }
            continue;
        }
        let erased = match column.shape {
            ErasureShape::Text => erase_exact(text, target),
            ErasureShape::AbsolutePath => erase_path(text, target)?,
        };
        if let Some((value, removed)) = erased {
            planned.push(ValueRedaction {
                column: column.name,
                value,
                removed,
            });
        }
    }
    Ok(planned)
}

fn erase_exact(text: &str, target: &str) -> Option<(String, u64)> {
    redact_exact(text, target)
}

fn erase_path(text: &str, target: &str) -> Result<Option<(String, u64)>, ErasurePageError> {
    let Some((redacted, removed)) = erase_exact(text, target) else {
        return Ok(None);
    };
    if redacted.is_empty() || !std::path::Path::new(&redacted).is_absolute() {
        if ERASED_LOCATOR.contains(target) {
            return Err(ErasurePageError::Unrepresentable);
        }
        return Ok(Some((String::from(ERASED_LOCATOR), removed)));
    }
    Ok(Some((redacted, removed)))
}

pub struct TaskErasureParticipant {
    core: ErasureCore,
}

impl TaskErasureParticipant {
    #[must_use]
    pub fn new(store: Store) -> Self {
        Self {
            core: ErasureCore::new(ParticipantOwnerRef::Task, TASK_STAGES, store),
        }
    }
}

impl ErasureParticipant for TaskErasureParticipant {
    fn owner(&self) -> ParticipantOwnerRef {
        ParticipantOwnerRef::Task
    }

    fn demand_local_erasure(
        &self,
        command: DemandLocalErasureCommand,
    ) -> Pin<Box<dyn std::future::Future<Output = ParticipantCompletionFact> + Send + '_>> {
        Box::pin(self.core.demand(command))
    }
}

pub struct ActionErasureParticipant {
    core: ErasureCore,
}

impl ActionErasureParticipant {
    #[must_use]
    pub fn new(store: Store) -> Self {
        Self {
            core: ErasureCore::new(ParticipantOwnerRef::Action, ACTION_STAGES, store),
        }
    }
}

impl ErasureParticipant for ActionErasureParticipant {
    fn owner(&self) -> ParticipantOwnerRef {
        ParticipantOwnerRef::Action
    }

    fn demand_local_erasure(
        &self,
        command: DemandLocalErasureCommand,
    ) -> Pin<Box<dyn std::future::Future<Output = ParticipantCompletionFact> + Send + '_>> {
        Box::pin(self.core.demand(command))
    }
}

pub struct InferenceErasureParticipant {
    core: ErasureCore,
}

impl InferenceErasureParticipant {
    #[must_use]
    pub fn new(store: Store) -> Self {
        Self {
            core: ErasureCore::new(ParticipantOwnerRef::Inference, INFERENCE_STAGES, store),
        }
    }
}

impl ErasureParticipant for InferenceErasureParticipant {
    fn owner(&self) -> ParticipantOwnerRef {
        ParticipantOwnerRef::Inference
    }

    fn demand_local_erasure(
        &self,
        command: DemandLocalErasureCommand,
    ) -> Pin<Box<dyn std::future::Future<Output = ParticipantCompletionFact> + Send + '_>> {
        Box::pin(self.core.demand(command))
    }
}
