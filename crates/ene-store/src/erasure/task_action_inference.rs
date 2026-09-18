//! Owner-local Targeted Deletion participants over this store's durable rows.
//!
//! `ene-preservation` owns the cross-cutting [`ErasureParticipant`] contract
//! and never depends on a concrete participant crate (lifecycle §9). The
//! Task, Action, and Inference owners keep their durable master in this
//! crate's single SQLite writer, so their bounded local-erasure
//! implementations live beside those tables, and the Host composition
//! (`apps/ene-core`) registers them with
//! `HostHandle::register_deletion_participant`.
//!
//! Every implementation is a mechanical, LLM-independent sweep over the
//! owner's body-bearing columns:
//!
//! * the exact target text is compared to every stored value (SQLite `instr`,
//!   no pattern grammar and no semantic matching);
//! * a match is redacted in place with a fixed marker, never by deleting the
//!   owner row, so the objective facts of the row (identity, revision,
//!   progress, adoption, certainty, grounds, ticket, provider, model, usage)
//!   stay exactly as committed;
//! * the work is bounded per demand by [`ROWS_PER_DEMAND`]; the fan-out
//!   re-demands until the participant reports `Verified`, and a crash or a
//!   restart re-drives the same sweep idempotently because a redaction is a
//!   no-op on an already-clean value;
//! * `Verified` is a second, complete pass over the same columns that found
//!   zero occurrences — never inferred from the erase pass alone.
//!
//! The reported `erased_count` / `remainder_count` describe the bounded work
//! this run observed; they are progress metadata, never the completion
//! decision (the durable `Verified` state plus the remainder pass is), and a
//! resumed run counts only what it redacts itself.
//!
//! The participant never decides deletion semantics: it receives the
//! protected exact-text material with the condition identity and reports the
//! fact. The covered source-correlation identities are not consulted here:
//! these owners' rows carry no reliable per-row source identity for a body
//! copy, so the exact text is the mechanical handle — and the same text is
//! what the bounded remainder pass verifies. A stale condition can only
//! produce a fact the canonical store refuses (`StaleSweep`), so an older
//! generation never advances the current sweep; a demand without protected
//! material or with a target the owner cannot represent is an explicit hold,
//! never a silent success.
//!
//! Owner surfaces:
//!
//! * Task: the in-force purpose text, every revision snapshot purpose, the
//!   recorded result body, the observation occurrence path correlation, and
//!   the internal workspace / delegation scope path copies. Correlation
//!   columns (`task_context_entry`, the observation identities, the delegation
//!   and association identities) are retained: they are body-free identities,
//!   not copies.
//! * Action: the attempt's resolved target path. Certainty and grounds are
//!   never rewritten; an already-observed external effect stays a fact, and
//!   only the stored target text is redacted.
//! * Inference: verification only. The current inference-owned surface
//!   (attempts, ordered `data_use` correlation, usage facts) stores no copy
//!   of a logical input or output body, so there is no body column to erase;
//!   the participant proves the absence over that empty set and never
//!   rewrites ticket, provider, model, usage, or pricing facts.

use std::collections::HashMap;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use ene_preservation::{
    DemandLocalErasureCommand, ErasureParticipant, MechanicalDeletionTarget,
    ParticipantCompletionFact, ParticipantHoldClass, ParticipantOwnerRef,
};
use ene_primitive::WallClockWithTz;
use rusqlite::{Connection, TransactionBehavior, params};

use crate::Store;
use crate::codec::lock_shared;
use crate::run_blocking;

use super::redact_exact;

/// Rows examined in one demand. The value bounds the SQL, the redaction work,
/// and the caller's wait; a longer sweep simply spans more demands.
pub(crate) const ROWS_PER_DEMAND: u32 = 64;

/// Rows fetched by one SQL page. Smaller than [`ROWS_PER_DEMAND`] so a demand
/// never depends on one statement reading its whole budget.
const PAGE_ROWS: u32 = 32;

/// The fixed locator stored when redacting a path would leave a value the
/// owner's own decode no longer accepts as a canonical absolute path
/// (`action_attempt.real_target`). It is a marker, never a route back to the
/// original target.
#[cfg(windows)]
pub(crate) const ERASED_LOCATOR: &str = r"C:\erased";
#[cfg(not(windows))]
pub(crate) const ERASED_LOCATOR: &str = "/erased";

/// How one column's value must remain shaped after a redaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ErasureShape {
    /// Free text: the redacted value itself is stored.
    Text,
    /// A filesystem locator: the redacted value must stay a canonical
    /// absolute path, otherwise the fixed [`ERASED_LOCATOR`] marker is used.
    AbsolutePath,
}

/// One body-bearing column of one owner row set.
#[derive(Debug, Clone, Copy)]
struct ErasureColumn {
    name: &'static str,
    shape: ErasureShape,
}

/// One row set whose columns are copied bodies subject to mechanical erasure.
///
/// Table and column names are compile-time constants and never caller input;
/// the target text only ever travels as a bound parameter.
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

/// The occurrence ledger's resolved workspace path correlation. It is a
/// locator copy (the same canonical AU5 target the Action owner records), so
/// it must keep the canonical absolute-path shape after a redaction exactly
/// like `action_attempt.real_target`. The occurrence identity, delegation /
/// task correlation, producing attempt identity, and body-observed marker are
/// facts and are never rewritten.
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

/// The Task owner's body-bearing row sets: the D1 in-force purpose, every D2
/// revision snapshot, the recorded final result, and the internal copies of
/// the workspace / delegation boundary. The context entries, correlation
/// columns, progress, revision, and adoption pointers are facts, not bodies.
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

/// The Action owner's target path. The attempt identity, delegation / task /
/// revision / workspace correlation, operation kind, certainty, grounds, and
/// started time are objective facts and are never rewritten.
const ACTION_STAGES: &[ErasureStage] = &[ErasureStage {
    table: "action_attempt",
    columns: &[ACTION_REAL_TARGET],
}];

/// The Inference owner has no body-bearing column in the current surface.
const INFERENCE_STAGES: &[ErasureStage] = &[];

/// Redacted value planned for one stored column.
struct ValueRedaction {
    column: &'static str,
    value: String,
    removed: u64,
}

/// Bounded progress of one `(operation, sweep)` sweep of one participant.
///
/// The cursor is participant-owned (lifecycle §9); it is in-memory only, so a
/// restart re-drives the same sweep from the start. Redaction is idempotent,
/// which is what makes that resume safe.
#[derive(Debug, Clone, Copy)]
struct SweepProgress {
    phase: SweepPhase,
    /// Index into the owner's stage list.
    stage: usize,
    /// Last rowid already processed in the current stage (exclusive cursor).
    after_rowid: i64,
    /// Values actually redacted in this sweep by this run.
    erased: u64,
    /// Occurrences found (and redacted) since the current verification pass
    /// started. A pass that ends with a non-zero count is not a clean pass.
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

/// One bounded chunk of work.
enum Chunk {
    /// The erase pass reached the end of every stage; the remainder check is
    /// still outstanding.
    EraseComplete { erased: u64 },
    /// The bounded work stopped before the current pass completed.
    MoreWork { erased: u64, remainder: u64 },
    /// A complete remainder pass found zero occurrences.
    Verified { erased: u64 },
}

/// One page of one stage, already redacted inside one short transaction.
struct ErasePage {
    last_rowid: i64,
    examined: u32,
    redacted: u64,
}

/// Failure of one bounded page. Both variants become explicit holds; neither
/// is ever a silent success and neither carries row content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ErasurePageError {
    /// The store could not read or write the page.
    Storage,
    /// The owner cannot store a target-free value its own read path accepts
    /// (the redacted path would not be a readable locator and the fixed
    /// marker itself contains the target). Fail closed instead of writing an
    /// unreadable row or a value that still contains the target.
    Unrepresentable,
}

/// Identity of one swept condition. The operation and generation travel
/// together: a later sweep is a different condition and never inherits
/// progress.
type DeletionSweepKey = (
    ene_preservation::DeletionOperationId,
    ene_preservation::DeletionSweepGeneration,
);

/// The shared bounded sweep of one owner surface.
struct ErasureCore {
    owner: ParticipantOwnerRef,
    stages: &'static [ErasureStage],
    store: Store,
    /// One entry per `(operation, sweep)` currently being swept. Entries are
    /// dropped when the sweep verifies, so the map tracks unfinished work.
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

    /// Runs one bounded demand and reports the fact. Every failure is an
    /// explicit hold: the caller's durable progress stays what it was.
    async fn demand(&self, command: DemandLocalErasureCommand) -> ParticipantCompletionFact {
        let condition = command.condition();
        let observed_at = WallClockWithTz::now();
        let Some(target) = exact_target(&command) else {
            // No protected material means this owner was demanded as a
            // correlation-only holder, which cannot happen for a local
            // semantic owner; it is a composition failure, not an empty sweep.
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
        match self.advance(progress, &target).await {
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
        }
    }

    /// Advances the sweep by at most [`ROWS_PER_DEMAND`] rows.
    async fn advance(
        &self,
        progress: &mut SweepProgress,
        target: &str,
    ) -> Result<Chunk, ErasurePageError> {
        let mut examined = 0u32;
        loop {
            if progress.stage >= self.stages.len() {
                match progress.phase {
                    SweepPhase::Erase => {
                        // The erase pass is complete; the remainder check is a
                        // separate, complete pass (lifecycle §10).
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
                        // The check found occurrences (already redacted), so it
                        // was not a clean pass: restart the whole check. Each
                        // restart strictly shrinks the remaining occurrences,
                        // so a sweep converges on a clean pass.
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
            let page = erase_page(&self.store, stage, target, progress.after_rowid, limit).await?;
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

/// The protected exact-text target of one demand, or [`None`] when the demand
/// carries no material or an empty/whitespace-only text (which the canonical
/// admission already refuses; a participant never guesses a broader scope).
fn exact_target(command: &DemandLocalErasureCommand) -> Option<String> {
    let target = command.scope().target()?;
    let MechanicalDeletionTarget::ExactText(material) = &target.mechanical;
    let exact = material.expose_for_erasure();
    if exact.trim().is_empty() {
        return None;
    }
    Some(exact.to_owned())
}

/// Runs one bounded page of one stage inside one short `Immediate`
/// transaction, so the read and the redactions of that page cannot interleave
/// with a concurrent writer.
async fn erase_page(
    store: &Store,
    stage: &'static ErasureStage,
    target: &str,
    after_rowid: i64,
    limit: u32,
) -> Result<ErasePage, ErasurePageError> {
    let conn = Arc::clone(&store.conn);
    let target = target.to_owned();
    run_blocking(move || erase_page_sync(&conn, stage, &target, after_rowid, limit)).await
}

fn erase_page_sync(
    conn: &Mutex<Connection>,
    stage: &ErasureStage,
    target: &str,
    after_rowid: i64,
    limit: u32,
) -> Result<ErasePage, ErasurePageError> {
    let mut guard = lock_shared(conn);
    let tx = guard
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|_| ErasurePageError::Storage)?;
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
        for value in plan_redactions(stage, row, target)? {
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

/// One stored row of one stage: its rowid cursor plus the stage's columns in
/// order. `None` is a stored NULL.
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
fn plan_redactions(
    stage: &ErasureStage,
    row: &RawErasureRow,
    target: &str,
) -> Result<Vec<ValueRedaction>, ErasurePageError> {
    let mut planned = Vec::new();
    for (column, value) in stage.columns.iter().zip(&row.values) {
        let Some(text) = value.as_deref() else {
            continue;
        };
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

/// Mechanically removes every occurrence of `target` from `text`.
///
/// The shared A3/A4 mechanical predicate ([`super::redact_exact`]): one
/// definition for the owner sweeps and the acceptance boundaries.
fn erase_exact(text: &str, target: &str) -> Option<(String, u64)> {
    redact_exact(text, target)
}

/// Redacts one stored filesystem locator, keeping it a readable canonical
/// absolute path. A redaction that would break that shape (the match consumed
/// the path root) is replaced by the fixed [`ERASED_LOCATOR`] marker; if even
/// the marker contains the target, the value is unrepresentable and the sweep
/// fails closed.
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

/// The Task owner's local-erasure participant.
pub struct TaskErasureParticipant {
    core: ErasureCore,
}

impl TaskErasureParticipant {
    /// Binds the participant to one store handle.
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

/// The Action owner's local-erasure participant.
pub struct ActionErasureParticipant {
    core: ErasureCore,
}

impl ActionErasureParticipant {
    /// Binds the participant to one store handle.
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

/// The Inference owner's verification participant.
pub struct InferenceErasureParticipant {
    core: ErasureCore,
}

impl InferenceErasureParticipant {
    /// Binds the participant to one store handle.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_erasure_removes_every_occurrence_and_reports_the_count() {
        let (redacted, removed) = erase_exact("keep the key here, key", "key").unwrap();
        assert_eq!(redacted, "keep the [erased] here, [erased]");
        assert_eq!(removed, 2);
        assert!(!redacted.contains("key"));
        assert_eq!(erase_exact("clean text", "key"), None);
        assert_eq!(erase_exact("text", ""), None);
    }

    #[test]
    fn exact_erasure_repeats_until_a_replacement_cannot_re_match() {
        // The first replacement leaves `d]` inside the marker itself, so a
        // single `replace` pass would report a clean value that still
        // contains the target.
        let (redacted, removed) = erase_exact("sd]x", "d]").unwrap();
        assert!(!redacted.contains("d]"), "got {redacted}");
        assert!(removed >= 2);
    }

    #[test]
    fn exact_erasure_falls_back_to_removal_when_the_target_overlaps_the_marker() {
        let target = "erased";
        let (redacted, removed) = erase_exact("please erase the erased marker", target).unwrap();
        assert!(!redacted.contains(target), "got {redacted}");
        assert!(removed >= 2);
    }

    #[test]
    fn path_erasure_keeps_an_absolute_locator() {
        let target = "/srv/secret";
        let Some((redacted, removed)) = erase_path("/srv/secret/notes.md", target).unwrap() else {
            unreachable!("the fixture contains the target");
        };
        assert!(!redacted.contains(target), "got {redacted}");
        assert!(
            std::path::Path::new(&redacted).is_absolute(),
            "got {redacted}"
        );
        assert_eq!(removed, 1);

        let untouched = erase_path("/srv/public/notes.md", target).unwrap();
        assert!(untouched.is_none());
    }

    #[test]
    fn path_erasure_reports_an_unrepresentable_value_instead_of_writing_the_target() {
        // The redaction consumes the root, so the fallback marker is used; the
        // marker itself contains this target, so the value cannot be stored
        // target-free and readable at once.
        let target = if cfg!(windows) {
            r"C:\erased"
        } else {
            "/erased"
        };
        assert_eq!(
            erase_path(target, target),
            Err(ErasurePageError::Unrepresentable)
        );
    }
}
