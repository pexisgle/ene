//! `LearningRepository` over the Learning table group.
//!
//! One commit writes the evidence Summary (once), the current Memory row, and
//! the new revision row in one short `Immediate` transaction. The expected
//! revision is compared inside that transaction, so a formation result
//! computed against an older recognition answers [`MemoryChangeOutcome::StaleTarget`]
//! and leaves the newer row untouched.

use std::sync::Arc;
use std::sync::Mutex;

use ene_learning::{
    ChangeKind, ExperienceSourceKind, Importance, LearningRepository, LearningScope,
    LearningTechnicalError, Memory, MemoryChange, MemoryChangeCommit, MemoryChangeOutcome,
    MemoryId, MemoryRevision, MemoryRevisionRecord, MemoryTarget, SourceRangeRef, SummaryId,
    SummaryRecord, TemporalMeaning,
};
use ene_primitive::{RawId, WallClockWithTz};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};

use crate::Store;
use crate::codec::{decode_id, decode_u64, encode_id, lock_shared};
use crate::credential::SQL_SELECT_SET_REV;
use crate::run_blocking;

const SQL_SELECT_MEMORY_TARGET: &str =
    "SELECT companion_id, revision FROM learning_memory WHERE memory_id = ?1";

const SQL_INSERT_MEMORY: &str = "INSERT INTO learning_memory (memory_id, companion_id, revision, content, importance, temporal, recall_suppressed, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)";

const SQL_UPDATE_MEMORY: &str = "UPDATE learning_memory SET revision = ?2, content = ?3, importance = ?4, temporal = ?5, recall_suppressed = ?6, updated_at = ?7 WHERE memory_id = ?1";

const SQL_INSERT_MEMORY_REVISION: &str = "INSERT INTO learning_memory_revision (memory_id, revision, companion_id, content, importance, temporal, recall_suppressed, change_kind, summary_id, at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)";

const SQL_INSERT_SUMMARY_IGNORE: &str = "INSERT OR IGNORE INTO learning_summary (summary_id, companion_id, content, source_kind, source_start, source_end, formed_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)";

const SQL_SELECT_CURRENT: &str = "SELECT memory_id, companion_id, revision, content, importance, temporal, recall_suppressed, updated_at FROM learning_memory WHERE memory_id = ?1";

const SQL_LIST_CURRENT: &str = "SELECT memory_id, companion_id, revision, content, importance, temporal, recall_suppressed, updated_at FROM learning_memory WHERE companion_id = ?1 ORDER BY rowid DESC LIMIT ?2";

const SQL_LIST_REVISIONS: &str = "SELECT memory_id, revision, companion_id, content, importance, temporal, recall_suppressed, change_kind, summary_id, at FROM learning_memory_revision WHERE memory_id = ?1 ORDER BY revision ASC";

const SQL_SELECT_SUMMARY: &str = "SELECT summary_id, companion_id, content, source_kind, source_start, source_end, formed_at FROM learning_summary WHERE summary_id = ?1";

fn learning_unavailable(reason: impl core::fmt::Display) -> LearningTechnicalError {
    LearningTechnicalError::StorageUnavailable {
        reason: reason.to_string(),
    }
}

fn commit_change_sync(
    conn: &Mutex<Connection>,
    commit: MemoryChangeCommit,
) -> Result<MemoryChangeOutcome, LearningTechnicalError> {
    let mut guard = lock_shared(conn);
    let tx = guard
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(learning_unavailable)?;
    // Credential-set premise first: content scrubbed before a credential
    // became registered must not be written, so the whole change (including
    // its Summary evidence) is refused before any row.
    if let Some(expected) = commit.secret_premise {
        let stored: i64 = tx
            .query_row(SQL_SELECT_SET_REV, (), |row| row.get(0))
            .map_err(learning_unavailable)?;
        let current = decode_u64(stored).map_err(learning_unavailable)?;
        if current != expected.as_u64() {
            return Ok(MemoryChangeOutcome::StaleCredentialSet);
        }
    }
    if let Some(summary) = &commit.summary {
        insert_summary(&tx, summary)?;
    }
    let change = &commit.change;
    let outcome = match change.target {
        MemoryTarget::New { id } => {
            let existing: Option<String> = tx
                .query_row(
                    "SELECT memory_id FROM learning_memory WHERE memory_id = ?1",
                    params![encode_id(id.as_raw())],
                    |row| row.get(0),
                )
                .optional()
                .map_err(learning_unavailable)?;
            if existing.is_some() {
                return Ok(MemoryChangeOutcome::AlreadyExists { memory: id });
            }
            let revision = MemoryRevision::initial();
            insert_current(&tx, id, revision, change)?;
            insert_revision(&tx, id, revision, change, commit.summary.as_ref())?;
            MemoryChangeOutcome::Committed {
                memory: id,
                revision,
            }
        }
        MemoryTarget::Existing {
            id,
            expected_revision,
        } => {
            let stored: Option<(String, i64)> = tx
                .query_row(
                    SQL_SELECT_MEMORY_TARGET,
                    params![encode_id(id.as_raw())],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()
                .map_err(learning_unavailable)?;
            let Some((stored_companion, stored_revision)) = stored else {
                return Ok(MemoryChangeOutcome::MissingTarget { memory: id });
            };
            if stored_companion != encode_id(change.scope.companion_id()) {
                return Ok(MemoryChangeOutcome::ScopeMismatch { memory: id });
            }
            let current = MemoryRevision::from_u64(
                decode_u64(stored_revision).map_err(learning_unavailable)?,
            );
            if current != expected_revision {
                return Ok(MemoryChangeOutcome::StaleTarget {
                    memory: id,
                    current,
                });
            }
            let Some(next) = current.checked_next() else {
                return Ok(MemoryChangeOutcome::RevisionExhausted { memory: id });
            };
            update_current(&tx, id, next, change)?;
            insert_revision(&tx, id, next, change, commit.summary.as_ref())?;
            MemoryChangeOutcome::Committed {
                memory: id,
                revision: next,
            }
        }
    };
    tx.commit().map_err(learning_unavailable)?;
    Ok(outcome)
}

fn insert_summary(
    tx: &Transaction<'_>,
    summary: &SummaryRecord,
) -> Result<(), LearningTechnicalError> {
    // Reused verbatim when several changes of one formation share it; the id
    // identifies the evidence, so a repeated insert is a no-op only while the
    // stored payload equals the offered one. A different payload under the
    // same identity would silently rebind the evidence, so it is refused and
    // the caller's transaction (including this insert) rolls back.
    tx.execute(
        SQL_INSERT_SUMMARY_IGNORE,
        params![
            encode_id(summary.id.as_raw()),
            encode_id(summary.scope.companion_id()),
            summary.content,
            encode_source_kind(summary.source.kind),
            encode_id(summary.source.start),
            encode_id(summary.source.end),
            summary.formed_at.to_rfc3339(),
        ],
    )
    .map_err(learning_unavailable)?;
    let stored = tx
        .query_row(
            SQL_SELECT_SUMMARY,
            params![encode_id(summary.id.as_raw())],
            |row| {
                Ok(RawSummary {
                    summary: row.get(0)?,
                    companion: row.get(1)?,
                    content: row.get(2)?,
                    source_kind: row.get(3)?,
                    source_start: row.get(4)?,
                    source_end: row.get(5)?,
                    formed_at: row.get(6)?,
                })
            },
        )
        .optional()
        .map_err(learning_unavailable)?
        .ok_or_else(|| learning_unavailable("summary vanished after insert"))?;
    if decode_summary(stored)? != *summary {
        return Err(LearningTechnicalError::SummaryIdentityConflict {
            summary: summary.id,
        });
    }
    Ok(())
}

fn insert_current(
    tx: &Transaction<'_>,
    memory: MemoryId,
    revision: MemoryRevision,
    change: &MemoryChange,
) -> Result<(), LearningTechnicalError> {
    tx.execute(
        SQL_INSERT_MEMORY,
        params![
            encode_id(memory.as_raw()),
            encode_id(change.scope.companion_id()),
            encode_revision(revision)?,
            change.content,
            encode_importance(change.importance),
            encode_temporal(change.temporal),
            i64::from(change.recall_suppressed),
            change.at.to_rfc3339(),
        ],
    )
    .map_err(learning_unavailable)?;
    Ok(())
}

fn update_current(
    tx: &Transaction<'_>,
    memory: MemoryId,
    revision: MemoryRevision,
    change: &MemoryChange,
) -> Result<(), LearningTechnicalError> {
    tx.execute(
        SQL_UPDATE_MEMORY,
        params![
            encode_id(memory.as_raw()),
            encode_revision(revision)?,
            change.content,
            encode_importance(change.importance),
            encode_temporal(change.temporal),
            i64::from(change.recall_suppressed),
            change.at.to_rfc3339(),
        ],
    )
    .map_err(learning_unavailable)?;
    Ok(())
}

fn insert_revision(
    tx: &Transaction<'_>,
    memory: MemoryId,
    revision: MemoryRevision,
    change: &MemoryChange,
    summary: Option<&SummaryRecord>,
) -> Result<(), LearningTechnicalError> {
    let summary_id = summary.map(|summary| encode_id(summary.id.as_raw()));
    tx.execute(
        SQL_INSERT_MEMORY_REVISION,
        params![
            encode_id(memory.as_raw()),
            encode_revision(revision)?,
            encode_id(change.scope.companion_id()),
            change.content,
            encode_importance(change.importance),
            encode_temporal(change.temporal),
            i64::from(change.recall_suppressed),
            encode_change(change.change),
            summary_id,
            change.at.to_rfc3339(),
        ],
    )
    .map_err(learning_unavailable)?;
    Ok(())
}

struct RawMemory {
    memory: String,
    companion: String,
    revision: i64,
    content: String,
    importance: i64,
    temporal: String,
    recall_suppressed: i64,
    updated_at: String,
}

struct RawRevision {
    memory: String,
    revision: i64,
    companion: String,
    content: String,
    importance: i64,
    temporal: String,
    recall_suppressed: i64,
    change: String,
    summary: Option<String>,
    at: String,
}

struct RawSummary {
    summary: String,
    companion: String,
    content: String,
    source_kind: String,
    source_start: String,
    source_end: String,
    formed_at: String,
}

fn decode_memory(raw: RawMemory) -> Result<Memory, LearningTechnicalError> {
    Ok(Memory {
        id: MemoryId::from_raw(decode_id(&raw.memory).map_err(learning_unavailable)?),
        revision: MemoryRevision::from_u64(decode_u64(raw.revision).map_err(learning_unavailable)?),
        scope: LearningScope::companion(decode_id(&raw.companion).map_err(learning_unavailable)?),
        content: raw.content,
        importance: decode_importance(raw.importance)?,
        temporal: decode_temporal(&raw.temporal)?,
        recall_suppressed: decode_suppressed(raw.recall_suppressed)?,
        updated_at: decode_clock(&raw.updated_at)?,
    })
}

fn decode_revision(raw: RawRevision) -> Result<MemoryRevisionRecord, LearningTechnicalError> {
    Ok(MemoryRevisionRecord {
        memory: MemoryId::from_raw(decode_id(&raw.memory).map_err(learning_unavailable)?),
        revision: MemoryRevision::from_u64(decode_u64(raw.revision).map_err(learning_unavailable)?),
        scope: LearningScope::companion(decode_id(&raw.companion).map_err(learning_unavailable)?),
        content: raw.content,
        importance: decode_importance(raw.importance)?,
        temporal: decode_temporal(&raw.temporal)?,
        change: decode_change(&raw.change)?,
        recall_suppressed: decode_suppressed(raw.recall_suppressed)?,
        summary: raw
            .summary
            .as_deref()
            .map(decode_id)
            .transpose()
            .map_err(learning_unavailable)?
            .map(SummaryId::from_raw),
        at: decode_clock(&raw.at)?,
    })
}

fn decode_summary(raw: RawSummary) -> Result<SummaryRecord, LearningTechnicalError> {
    Ok(SummaryRecord {
        id: SummaryId::from_raw(decode_id(&raw.summary).map_err(learning_unavailable)?),
        scope: LearningScope::companion(decode_id(&raw.companion).map_err(learning_unavailable)?),
        content: raw.content,
        source: SourceRangeRef {
            kind: decode_source_kind(&raw.source_kind)?,
            start: decode_id(&raw.source_start).map_err(learning_unavailable)?,
            end: decode_id(&raw.source_end).map_err(learning_unavailable)?,
        },
        formed_at: decode_clock(&raw.formed_at)?,
    })
}

fn load_current_sync(
    conn: &Mutex<Connection>,
    memory: MemoryId,
) -> Result<Option<Memory>, LearningTechnicalError> {
    let guard = lock_shared(conn);
    let row = guard
        .query_row(
            SQL_SELECT_CURRENT,
            params![encode_id(memory.as_raw())],
            raw_memory_row,
        )
        .optional()
        .map_err(learning_unavailable)?;
    row.map(decode_memory).transpose()
}

fn list_current_sync(
    conn: &Mutex<Connection>,
    companion: RawId,
    limit: u64,
) -> Result<Vec<Memory>, LearningTechnicalError> {
    let guard = lock_shared(conn);
    let mut statement = guard
        .prepare(SQL_LIST_CURRENT)
        .map_err(learning_unavailable)?;
    let rows = statement
        .query_map(
            params![encode_id(companion), encode_limit(limit)?],
            raw_memory_row,
        )
        .map_err(learning_unavailable)?;
    let mut memories = Vec::new();
    for row in rows {
        memories.push(decode_memory(row.map_err(learning_unavailable)?)?);
    }
    Ok(memories)
}

fn list_revisions_sync(
    conn: &Mutex<Connection>,
    memory: MemoryId,
) -> Result<Vec<MemoryRevisionRecord>, LearningTechnicalError> {
    let guard = lock_shared(conn);
    let mut statement = guard
        .prepare(SQL_LIST_REVISIONS)
        .map_err(learning_unavailable)?;
    let rows = statement
        .query_map(params![encode_id(memory.as_raw())], |row| {
            Ok(RawRevision {
                memory: row.get(0)?,
                revision: row.get(1)?,
                companion: row.get(2)?,
                content: row.get(3)?,
                importance: row.get(4)?,
                temporal: row.get(5)?,
                recall_suppressed: row.get(6)?,
                change: row.get(7)?,
                summary: row.get(8)?,
                at: row.get(9)?,
            })
        })
        .map_err(learning_unavailable)?;
    let mut revisions = Vec::new();
    for row in rows {
        revisions.push(decode_revision(row.map_err(learning_unavailable)?)?);
    }
    Ok(revisions)
}

fn load_summary_sync(
    conn: &Mutex<Connection>,
    summary: SummaryId,
) -> Result<Option<SummaryRecord>, LearningTechnicalError> {
    let guard = lock_shared(conn);
    let row = guard
        .query_row(
            SQL_SELECT_SUMMARY,
            params![encode_id(summary.as_raw())],
            |row| {
                Ok(RawSummary {
                    summary: row.get(0)?,
                    companion: row.get(1)?,
                    content: row.get(2)?,
                    source_kind: row.get(3)?,
                    source_start: row.get(4)?,
                    source_end: row.get(5)?,
                    formed_at: row.get(6)?,
                })
            },
        )
        .optional()
        .map_err(learning_unavailable)?;
    row.map(decode_summary).transpose()
}

fn raw_memory_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RawMemory> {
    Ok(RawMemory {
        memory: row.get(0)?,
        companion: row.get(1)?,
        revision: row.get(2)?,
        content: row.get(3)?,
        importance: row.get(4)?,
        temporal: row.get(5)?,
        recall_suppressed: row.get(6)?,
        updated_at: row.get(7)?,
    })
}

fn encode_limit(limit: u64) -> Result<i64, LearningTechnicalError> {
    i64::try_from(limit).map_err(learning_unavailable)
}

fn encode_revision(revision: MemoryRevision) -> Result<i64, LearningTechnicalError> {
    i64::try_from(revision.as_u64()).map_err(learning_unavailable)
}

fn decode_clock(text: &str) -> Result<WallClockWithTz, LearningTechnicalError> {
    WallClockWithTz::parse_rfc3339(text).map_err(learning_unavailable)
}

fn encode_importance(importance: Importance) -> i64 {
    i64::from(importance.as_u8())
}

fn decode_importance(raw: i64) -> Result<Importance, LearningTechnicalError> {
    let value = u8::try_from(raw).map_err(learning_unavailable)?;
    if !(Importance::MIN..=Importance::MAX).contains(&value) {
        return Err(learning_unavailable("importance out of range"));
    }
    Ok(Importance::clamped(value))
}

fn decode_suppressed(raw: i64) -> Result<bool, LearningTechnicalError> {
    match raw {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(learning_unavailable("malformed recall suppression flag")),
    }
}

fn encode_temporal(temporal: TemporalMeaning) -> &'static str {
    match temporal {
        TemporalMeaning::Enduring => "enduring",
        TemporalMeaning::Event => "event",
    }
}

fn decode_temporal(text: &str) -> Result<TemporalMeaning, LearningTechnicalError> {
    match text {
        "enduring" => Ok(TemporalMeaning::Enduring),
        "event" => Ok(TemporalMeaning::Event),
        _ => Err(learning_unavailable("unknown temporal meaning")),
    }
}

fn encode_change(change: ChangeKind) -> &'static str {
    match change {
        ChangeKind::Initial => "initial",
        ChangeKind::Reinforced => "reinforced",
        ChangeKind::Refined => "refined",
        ChangeKind::Integrated => "integrated",
        ChangeKind::CorrectedInitiallyWrong => "corrected_initially_wrong",
        ChangeKind::ChangedSince => "changed_since",
        ChangeKind::Forgotten => "forgotten",
    }
}

fn decode_change(text: &str) -> Result<ChangeKind, LearningTechnicalError> {
    match text {
        "initial" => Ok(ChangeKind::Initial),
        "reinforced" => Ok(ChangeKind::Reinforced),
        "refined" => Ok(ChangeKind::Refined),
        "integrated" => Ok(ChangeKind::Integrated),
        "corrected_initially_wrong" => Ok(ChangeKind::CorrectedInitiallyWrong),
        "changed_since" => Ok(ChangeKind::ChangedSince),
        "forgotten" => Ok(ChangeKind::Forgotten),
        _ => Err(learning_unavailable("unknown change kind")),
    }
}

fn encode_source_kind(kind: ExperienceSourceKind) -> &'static str {
    match kind {
        ExperienceSourceKind::Dialogue => "dialogue",
    }
}

fn decode_source_kind(text: &str) -> Result<ExperienceSourceKind, LearningTechnicalError> {
    match text {
        "dialogue" => Ok(ExperienceSourceKind::Dialogue),
        _ => Err(learning_unavailable("unknown experience source kind")),
    }
}

impl LearningRepository for Store {
    async fn commit_memory_change(
        &self,
        commit: MemoryChangeCommit,
    ) -> Result<MemoryChangeOutcome, LearningTechnicalError> {
        let conn = Arc::clone(&self.conn);
        run_blocking(move || commit_change_sync(&conn, commit)).await
    }

    async fn load_current_memory(
        &self,
        memory: MemoryId,
    ) -> Result<Option<Memory>, LearningTechnicalError> {
        let conn = Arc::clone(&self.conn);
        run_blocking(move || load_current_sync(&conn, memory)).await
    }

    async fn list_current_memories(
        &self,
        companion: RawId,
        limit: u64,
    ) -> Result<Vec<Memory>, LearningTechnicalError> {
        let conn = Arc::clone(&self.conn);
        run_blocking(move || list_current_sync(&conn, companion, limit)).await
    }

    async fn list_memory_revisions(
        &self,
        memory: MemoryId,
    ) -> Result<Vec<MemoryRevisionRecord>, LearningTechnicalError> {
        let conn = Arc::clone(&self.conn);
        run_blocking(move || list_revisions_sync(&conn, memory)).await
    }

    async fn load_summary(
        &self,
        summary: SummaryId,
    ) -> Result<Option<SummaryRecord>, LearningTechnicalError> {
        let conn = Arc::clone(&self.conn);
        run_blocking(move || load_summary_sync(&conn, summary)).await
    }
}
