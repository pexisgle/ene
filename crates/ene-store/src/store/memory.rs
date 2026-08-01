//! Typed-memory, memory-embedding, memory-span, and pending-write queries.

use super::{
    EneMemoryError, MemoryStore, NaturalDecayReport, NewMemorySpan, embedding_to_bytes,
    validate_embedding,
};
use crate::entities;
use chrono::{DateTime, Utc};
use ene_core::{ActiveSceneSummaryRow, PendingMemoryWrite, PendingMemoryWriteStatus};
use sea_orm::sea_query::Expr;
use sea_orm::{ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter};

/// Memory statuses eligible for hybrid recall: `active`, `faded`, and `disputed`.
///
/// Archived / superseded / user-deleted memories are excluded from recall
/// unless a caller explicitly opts in (see `list_typed_memories`).
const RECALLABLE_STATUSES: [&str; 3] = [
    crate::MemoryStatus::Active.as_str(),
    crate::MemoryStatus::Faded.as_str(),
    crate::MemoryStatus::Disputed.as_str(),
];

/// Builds the user-visibility filter for typed-memory queries.
///
/// A memory is visible to `user_id` when it is owned by that user **or**
/// has no owner (empty `user_id`), so shared / character-level memories
/// remain recallable across users.
fn user_visibility_condition(user_id: &str) -> sea_orm::Condition {
    sea_orm::Condition::any()
        .add(entities::typed_memories::Column::UserId.eq(user_id))
        .add(entities::typed_memories::Column::UserId.eq(""))
}

fn escape_sql_literal(value: &str) -> String {
    value.replace('\'', "''")
}

async fn query_sqlite_changes(db: &sea_orm::DatabaseConnection) -> Result<usize, sea_orm::DbErr> {
    use sea_orm::{ConnectionTrait, DatabaseBackend, Statement};
    let row = db
        .query_one_raw(Statement::from_string(
            DatabaseBackend::Sqlite,
            "SELECT changes() AS n",
        ))
        .await?;
    let count = row
        .and_then(|r| r.try_get::<i64>("", "n").ok())
        .unwrap_or(0);
    Ok(usize::try_from(count).unwrap_or(0))
}

/// Strip the trailing `ene:tags` metadata footer from memory content.
///
/// The memory arbiter appends `\n\n<!-- ene:tags {"tags":[...]} -->` at
/// write time. This footer is internal metadata and must not leak into
/// LLM prompts or recall scoring. Stripping at the model-to-item
/// conversion layer covers all readers with a single choke point.
///
/// Visibility is `pub(super)` (not `pub(crate)`) so that the sibling
/// `store::tests` module can exercise this function directly while keeping
/// it hidden from external callers.
pub(super) fn strip_tags_footer(content: &str) -> &str {
    match content.find("\n\n<!-- ene:tags ") {
        #[expect(
            clippy::string_slice,
            reason = "find(...) matches ASCII string '\\n\\n<!-- ene:tags ' guaranteeing valid UTF-8 boundary"
        )]
        Some(pos) => &content[..pos],
        None => content,
    }
}

/// Convert a typed memory model row to a [`crate::MemoryItem`].
#[expect(
    clippy::unnecessary_wraps,
    reason = "store helper signature returns Result for uniform error propagation"
)]
fn model_to_memory_item(
    m: entities::typed_memories::Model,
) -> Result<crate::MemoryItem, EneMemoryError> {
    Ok(crate::MemoryItem {
        id: Some(m.id),
        scope: crate::MemoryScope::from_db_str(&m.scope),
        character_id: m.character_id,
        user_id: m.user_id,
        kind: crate::MemoryKind::from_db_str(&m.kind),
        title: m.title,
        content: strip_tags_footer(&m.content).to_owned(),
        source: crate::MemorySource::from_db_str(&m.source),
        source_ref: m.source_ref,
        confidence: crate::MemoryConfidence::new(m.confidence),
        salience: crate::MemorySalience::new(m.salience),
        affect: crate::AffectAnnotation {
            valence: m.affective_valence,
            arousal: m.affective_arousal,
        },
        relationship_impact: m.relationship_impact,
        access_count: m.access_count,
        last_accessed_at: m.last_accessed_at,
        created_at: m.created_at,
        updated_at: m.updated_at,
        valid_from: m.valid_from,
        valid_until: m.valid_until,
        status: crate::MemoryStatus::from_db_str(&m.status),
        supersedes_id: m.supersedes_id,
        pinned: m.pinned != 0,
        faded_at: m.faded_at,
        commitment_id: m.commitment_id,
    })
}

const fn is_supersedeable_status(status: crate::MemoryStatus) -> bool {
    matches!(
        status,
        crate::MemoryStatus::Active | crate::MemoryStatus::Faded | crate::MemoryStatus::Disputed
    )
}

fn merge_hybrid_candidate(
    gathered: &mut std::collections::HashMap<i64, ene_core::GatheredCandidate>,
    user_id: Option<&str>,
    exclude_kinds: &[ene_core::MemoryKind],
    item: crate::MemoryItem,
    vector_similarity: f32,
    source: crate::MemoryCandidateSource,
) {
    use ene_core::GatheredCandidate;
    use ene_rag::is_recallable_status;

    if !is_recallable_status(item.status) {
        return;
    }
    if exclude_kinds.contains(&item.kind) {
        return;
    }
    if let Some(uid) = user_id
        && !item.user_id.is_empty()
        && item.user_id != uid
    {
        return;
    }
    let Some(id) = item.id else {
        return;
    };
    gathered
        .entry(id)
        .and_modify(|candidate| {
            candidate.vector_similarity = candidate.vector_similarity.max(vector_similarity);
            if !candidate.sources.contains(&source) {
                candidate.sources.push(source);
            }
        })
        .or_insert_with(|| GatheredCandidate {
            item,
            vector_similarity,
            sources: vec![source],
        });
}

async fn list_session_ids_for_card_on_conn<C: ConnectionTrait>(
    conn: &C,
    card_name: &str,
) -> Result<Vec<String>, EneMemoryError> {
    use sea_orm::QuerySelect;

    let rows = entities::conversation_logs::Entity::find()
        .filter(entities::conversation_logs::Column::CardName.eq(card_name))
        .select_only()
        .column(entities::conversation_logs::Column::SessionId)
        .distinct()
        .into_tuple::<String>()
        .all(conn)
        .await?;
    Ok(rows)
}

impl MemoryStore {
    // ── Pending memory writes (#240) ────────────────────────────────────────

    /// Default maximum retry attempts for a deferred memory write.
    pub const PENDING_MEMORY_WRITE_MAX_ATTEMPTS: i32 = 5;

    /// Enqueue a failed deferred memory write for later retry (#240).
    pub async fn enqueue_pending_memory_write(
        &self,
        character_id: &str,
        user_id: &str,
        payload_json: impl Into<String>,
        last_error: impl Into<String>,
    ) -> Result<i64, EneMemoryError> {
        use entities::pending_memory_writes::ActiveModel;
        use sea_orm::ActiveModelTrait;
        use sea_orm::ActiveValue::Set;

        let now = Utc::now();
        let backoff = chrono::Duration::seconds(30);
        let next_retry = now.checked_add_signed(backoff).unwrap_or(now);
        let active = ActiveModel {
            id: sea_orm::NotSet,
            character_id: Set(character_id.to_string()),
            user_id: Set(user_id.to_string()),
            payload_json: Set(payload_json.into()),
            attempts: Set(1),
            max_attempts: Set(Self::PENDING_MEMORY_WRITE_MAX_ATTEMPTS),
            last_error: Set(Some(last_error.into())),
            status: Set(PendingMemoryWriteStatus::Pending.as_str().to_string()),
            created_at: Set(now),
            next_retry_at: Set(next_retry),
            updated_at: Set(now),
        };
        let res = active.insert(&self.db).await?;
        Ok(res.id)
    }

    /// List pending / permanent memory-write rows for a character (#240).
    pub async fn list_pending_memory_writes(
        &self,
        character_id: &str,
        limit: usize,
    ) -> Result<Vec<PendingMemoryWrite>, EneMemoryError> {
        use entities::pending_memory_writes::{Column, Entity};
        use sea_orm::{EntityTrait, QueryFilter, QueryOrder, QuerySelect};

        let rows = Entity::find()
            .filter(Column::CharacterId.eq(character_id))
            .order_by_desc(Column::UpdatedAt)
            .limit(limit as u64)
            .all(&self.db)
            .await?;
        Ok(rows.into_iter().map(Into::into).collect())
    }

    /// Count pending (retryable) and permanent failed memory writes (#240).
    pub async fn count_pending_memory_writes(
        &self,
        character_id: &str,
    ) -> Result<(usize, usize), EneMemoryError> {
        use entities::pending_memory_writes::{Column, Entity};
        use sea_orm::{ColumnTrait, EntityTrait, PaginatorTrait, QueryFilter};

        let pending = Entity::find()
            .filter(Column::CharacterId.eq(character_id))
            .filter(Column::Status.eq(PendingMemoryWriteStatus::Pending.as_str()))
            .count(&self.db)
            .await? as usize;
        let permanent = Entity::find()
            .filter(Column::CharacterId.eq(character_id))
            .filter(Column::Status.eq(PendingMemoryWriteStatus::Permanent.as_str()))
            .count(&self.db)
            .await? as usize;
        Ok((pending, permanent))
    }

    /// Force pending rows for a character to be due immediately (#240).
    ///
    /// Used by `/memory retry` so the operator can drain the queue without
    /// waiting for exponential backoff.
    pub async fn schedule_pending_memory_writes_now(
        &self,
        character_id: &str,
    ) -> Result<usize, EneMemoryError> {
        use entities::pending_memory_writes::{Column, Entity};
        use sea_orm::{EntityTrait, QueryFilter};

        let now = Utc::now();
        let result = Entity::update_many()
            .col_expr(Column::NextRetryAt, sea_orm::sea_query::Expr::value(now))
            .col_expr(Column::UpdatedAt, sea_orm::sea_query::Expr::value(now))
            .filter(Column::CharacterId.eq(character_id))
            .filter(Column::Status.eq(PendingMemoryWriteStatus::Pending.as_str()))
            .exec(&self.db)
            .await?;

        Ok(result.rows_affected as usize)
    }

    /// Take due pending memory writes (`status=pending`, `next_retry_at` <= now) (#240).
    pub async fn take_due_pending_memory_writes(
        &self,
        limit: usize,
    ) -> Result<Vec<PendingMemoryWrite>, EneMemoryError> {
        use entities::pending_memory_writes::{ActiveModel, Column, Entity};
        use sea_orm::{
            ActiveModelTrait, EntityTrait, QueryFilter, QueryOrder, QuerySelect, TransactionTrait,
        };

        let now = Utc::now();
        let txn = self.db.begin().await?;
        let rows = Entity::find()
            .filter(Column::Status.eq(PendingMemoryWriteStatus::Pending.as_str()))
            .filter(Column::NextRetryAt.lte(now))
            .order_by_asc(Column::NextRetryAt)
            .limit(limit as u64)
            .all(&txn)
            .await?;

        // Mark them as in-flight by bumping next_retry_at so concurrent drainers
        // do not pick the same rows (simple lease).
        let lease = now
            .checked_add_signed(chrono::Duration::minutes(5))
            .unwrap_or(now);
        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            let mut active: ActiveModel = row.clone().into();
            active.next_retry_at = sea_orm::Set(lease);
            active.updated_at = sea_orm::Set(now);
            active.update(&txn).await?;
            out.push(row.into());
        }
        txn.commit().await?;
        Ok(out)
    }

    /// Mark a pending memory write as successfully applied and delete it (#240).
    pub async fn complete_pending_memory_write(&self, id: i64) -> Result<(), EneMemoryError> {
        use entities::pending_memory_writes::Entity;
        use sea_orm::EntityTrait;

        Entity::delete_by_id(id).exec(&self.db).await?;
        Ok(())
    }

    /// Record another failed attempt; may transition to permanent (#240).
    pub async fn fail_pending_memory_write(
        &self,
        id: i64,
        last_error: impl Into<String>,
    ) -> Result<PendingMemoryWrite, EneMemoryError> {
        use entities::pending_memory_writes::{ActiveModel, Entity};
        use sea_orm::{ActiveModelTrait, EntityTrait};

        let Some(model) = Entity::find_by_id(id).one(&self.db).await? else {
            return Err(EneMemoryError::Other(format!(
                "pending memory write id={id} not found"
            )));
        };
        let attempts = model.attempts.saturating_add(1);
        let permanent = attempts >= model.max_attempts;
        let now = Utc::now();
        // Exponential backoff: 30s * 2^(attempts-1), capped at 1 hour.
        let delay_secs = 30i64
            .saturating_mul(1i64 << (attempts.saturating_sub(1).min(10) as u32))
            .min(3600);
        let mut active: ActiveModel = model.into();
        active.attempts = sea_orm::Set(attempts);
        active.last_error = sea_orm::Set(Some(last_error.into()));
        active.status = sea_orm::Set(
            if permanent {
                PendingMemoryWriteStatus::Permanent
            } else {
                PendingMemoryWriteStatus::Pending
            }
            .as_str()
            .to_string(),
        );
        active.next_retry_at = sea_orm::Set(
            now.checked_add_signed(chrono::Duration::seconds(delay_secs))
                .unwrap_or(now),
        );
        active.updated_at = sea_orm::Set(now);
        let updated = active.update(&self.db).await?;
        Ok(updated.into())
    }

    // ── Typed Memory CRUD ───────────────────────────────────────────────────

    /// Insert a new typed memory item and return its assigned ID.
    pub async fn insert_typed_memory(
        &self,
        item: &crate::NewMemoryItem,
    ) -> Result<i64, EneMemoryError> {
        use sea_orm::ActiveModelTrait;
        use sea_orm::ActiveValue::Set;

        let now = Utc::now();
        let created_at = item.created_at.unwrap_or(now);
        let active = entities::typed_memories::ActiveModel {
            scope: Set(item.scope.as_str().to_string()),
            character_id: Set(item.character_id.clone()),
            user_id: Set(item.user_id.clone()),
            kind: Set(item.kind.as_str().to_string()),
            title: Set(item.title.clone()),
            content: Set(item.content.clone()),
            source: Set(item.source.as_str().to_string()),
            source_ref: Set(item.source_ref.clone()),
            confidence: Set(item.confidence.get()),
            salience: Set(item.salience.get()),
            affective_valence: Set(item.affect.valence),
            affective_arousal: Set(item.affect.arousal),
            relationship_impact: Set(item.relationship_impact),
            access_count: Set(0),
            last_accessed_at: Set(None),
            created_at: Set(created_at),
            updated_at: Set(now),
            valid_from: Set(item.valid_from),
            valid_until: Set(item.valid_until),
            status: Set(item.status.as_str().to_string()),
            supersedes_id: Set(item.supersedes_id),
            pinned: Set(i32::from(item.pinned)),
            commitment_id: Set(item.commitment_id),
            ..Default::default()
        };
        let res = active.insert(&self.db).await?;
        Ok(res.id)
    }

    /// Retrieve a typed memory item by its ID.
    pub async fn get_typed_memory(
        &self,
        id: i64,
    ) -> Result<Option<crate::MemoryItem>, EneMemoryError> {
        let maybe_model = entities::typed_memories::Entity::find_by_id(id)
            .one(&self.db)
            .await?;
        match maybe_model {
            Some(m) => model_to_memory_item(m).map(Some),
            None => Ok(None),
        }
    }

    /// List typed memories for a character, optionally filtered by kind.
    pub async fn get_typed_memories_by_character(
        &self,
        character_id: &str,
        kind: Option<crate::MemoryKind>,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<crate::MemoryItem>, EneMemoryError> {
        use sea_orm::{EntityTrait, QueryFilter, QueryOrder, QuerySelect};

        let mut query = entities::typed_memories::Entity::find()
            .filter(entities::typed_memories::Column::CharacterId.eq(character_id));

        if let Some(k) = kind {
            query = query.filter(entities::typed_memories::Column::Kind.eq(k.as_str()));
        }

        let models = query
            .order_by_desc(entities::typed_memories::Column::CreatedAt)
            .limit(limit as u64)
            .offset(offset as u64)
            .all(&self.db)
            .await?;

        models
            .into_iter()
            .map(model_to_memory_item)
            .collect::<Result<Vec<_>, _>>()
    }

    /// List the namespaced tool names with a recent, recallable
    /// `tool failure:{tool}` reflection memory for a character (#349).
    ///
    /// Backs [`ene_core::ToolFailureSignalPort`] so the tool-selection RAG
    /// pipeline can down-weight tools that recently failed, without depending
    /// on this crate. Each tool name appears at most once, ordered by the most
    /// recent failure first.
    pub async fn recent_tool_failures(
        &self,
        character_id: &str,
    ) -> Result<Vec<String>, EneMemoryError> {
        use sea_orm::{EntityTrait, QueryFilter, QueryOrder};

        let models = entities::typed_memories::Entity::find()
            .filter(entities::typed_memories::Column::CharacterId.eq(character_id))
            .filter(
                entities::typed_memories::Column::Kind.eq(crate::MemoryKind::Reflection.as_str()),
            )
            .filter(entities::typed_memories::Column::Status.is_in([
                crate::MemoryStatus::Active.as_str(),
                crate::MemoryStatus::Faded.as_str(),
            ]))
            .filter(entities::typed_memories::Column::Title.starts_with("tool failure:"))
            .order_by_desc(entities::typed_memories::Column::CreatedAt)
            .all(&self.db)
            .await?;

        let mut seen = std::collections::HashSet::new();
        let mut tools = Vec::new();
        for model in models {
            if let Some(tool) = model.title.strip_prefix("tool failure:")
                && seen.insert(tool.to_string())
            {
                tools.push(tool.to_string());
            }
        }
        Ok(tools)
    }

    /// Count typed memories for a character, optionally filtered by kind.
    pub async fn count_typed_memories(
        &self,
        character_id: &str,
        kind: Option<crate::MemoryKind>,
    ) -> Result<i64, EneMemoryError> {
        use sea_orm::{EntityTrait, PaginatorTrait, QueryFilter};

        let mut query = entities::typed_memories::Entity::find()
            .filter(entities::typed_memories::Column::CharacterId.eq(character_id));

        if let Some(k) = kind {
            query = query.filter(entities::typed_memories::Column::Kind.eq(k.as_str()));
        }

        Ok(query.count(&self.db).await? as i64)
    }

    /// List active typed memories whose `source_ref` starts with `prefix`.
    pub async fn list_typed_memories_by_source_prefix(
        &self,
        character_id: &str,
        prefix: &str,
        limit: usize,
    ) -> Result<Vec<crate::MemoryItem>, EneMemoryError> {
        use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QueryOrder, QuerySelect};

        let models = entities::typed_memories::Entity::find()
            .filter(entities::typed_memories::Column::CharacterId.eq(character_id))
            .filter(
                entities::typed_memories::Column::Status.eq(crate::MemoryStatus::Active.as_str()),
            )
            .filter(entities::typed_memories::Column::SourceRef.starts_with(prefix))
            .order_by_desc(entities::typed_memories::Column::Salience)
            .limit(limit as u64)
            .all(&self.db)
            .await?;

        models
            .into_iter()
            .map(model_to_memory_item)
            .collect::<Result<Vec<_>, _>>()
    }

    /// Returns whether an active typed memory exists for `character_id` + `source_ref`.
    pub async fn typed_memory_exists_by_source_ref(
        &self,
        character_id: &str,
        source_ref: &str,
    ) -> Result<bool, EneMemoryError> {
        Ok(self
            .get_active_typed_memory_by_source_ref(character_id, source_ref)
            .await?
            .is_some())
    }

    /// Returns the active typed memory for `character_id` + `source_ref`, if any.
    pub async fn get_active_typed_memory_by_source_ref(
        &self,
        character_id: &str,
        source_ref: &str,
    ) -> Result<Option<crate::MemoryItem>, EneMemoryError> {
        use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QuerySelect};

        let model = entities::typed_memories::Entity::find()
            .filter(entities::typed_memories::Column::CharacterId.eq(character_id))
            .filter(entities::typed_memories::Column::SourceRef.eq(source_ref))
            .filter(
                entities::typed_memories::Column::Status.eq(crate::MemoryStatus::Active.as_str()),
            )
            .limit(1)
            .one(&self.db)
            .await?;

        match model {
            Some(m) => model_to_memory_item(m).map(Some),
            None => Ok(None),
        }
    }

    /// Archive active typed memories under `prefixes` whose `source_ref` is not kept.
    pub async fn archive_typed_memories_by_source_prefixes(
        &self,
        character_id: &str,
        prefixes: &[&str],
        keep_refs: &std::collections::HashSet<String>,
    ) -> Result<usize, EneMemoryError> {
        use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};

        let mut archived = 0usize;
        for prefix in prefixes {
            let models = entities::typed_memories::Entity::find()
                .filter(entities::typed_memories::Column::CharacterId.eq(character_id))
                .filter(
                    entities::typed_memories::Column::Status
                        .eq(crate::MemoryStatus::Active.as_str()),
                )
                .filter(entities::typed_memories::Column::SourceRef.starts_with(*prefix))
                .all(&self.db)
                .await?;

            for model in models {
                let Some(source_ref) = model.source_ref else {
                    continue;
                };
                if keep_refs.contains(&source_ref) {
                    continue;
                }
                self.set_memory_status(model.id, crate::MemoryStatus::Faded)
                    .await?;
                self.set_memory_status(model.id, crate::MemoryStatus::Archived)
                    .await?;
                archived = archived.saturating_add(1);
            }
        }
        Ok(archived)
    }

    /// Gather typed-memory candidates for explainable hybrid scoring (#123, #302).
    ///
    /// Sole public typed-memory gather entry. Collects candidates from optional
    /// vector similarity (when `query.embedding` is `Some`), lexical token
    /// matches, a limited recent fallback, and active commitments, then
    /// de-duplicates by memory id. **Scoring, filtering, sorting, and
    /// truncation are the `ene-rag` layer's job** — callers compose
    /// `store.search(...)` with [`ene_rag::score_and_rank`] to get ranked
    /// [`crate::ScoredMemory`] results. Callers must pre-compute embeddings —
    /// the store never embeds.
    pub async fn search(
        &self,
        query: &crate::Query<'_>,
    ) -> Result<Vec<ene_core::GatheredCandidate>, EneMemoryError> {
        use crate::typed_memory::MemoryCandidateSource;
        use ene_rag::lexical_overlap_score;
        use std::collections::HashMap;

        if let Some(embedding) = query.embedding {
            validate_embedding(embedding, self.embedding_dim)?;
        }

        let pool = query.candidate_pool_size.max(query.limit);
        let mut gathered: HashMap<i64, ene_core::GatheredCandidate> = HashMap::new();

        // Vector candidates across recallable statuses (skipped when no embedding).
        if let Some(embedding) = query.embedding {
            let vector_hits = self
                .search_typed_memories_vector(
                    embedding,
                    query.character_id,
                    query.model_name,
                    query.user_id,
                    &RECALLABLE_STATUSES,
                    pool,
                    query.similarity_threshold,
                )
                .await?;
            for (item, similarity) in vector_hits {
                merge_hybrid_candidate(
                    &mut gathered,
                    query.user_id,
                    &query.exclude_kinds,
                    item,
                    similarity,
                    MemoryCandidateSource::Vector,
                );
            }
        }

        // Lexical candidates from token-based DB lookup.
        let lexical_candidates = self
            .list_lexical_typed_memory_candidates(
                query.query_text,
                query.character_id,
                query.user_id,
                pool,
            )
            .await?;
        for item in lexical_candidates {
            let lexical = lexical_overlap_score(query.query_text, &item.title, &item.content);
            if lexical > 0.0 {
                merge_hybrid_candidate(
                    &mut gathered,
                    query.user_id,
                    &query.exclude_kinds,
                    item,
                    0.0,
                    MemoryCandidateSource::Lexical,
                );
            }
        }

        // Active commitment ledger rows are the SoT for Commitment boost (#124).
        // Prefer typed rows linked via commitment_id; otherwise synthesize from ledger.
        let commitments = self
            .list_active_commitments(query.character_id, query.user_id, pool)
            .await?;
        let commitment_ids: Vec<i64> = commitments.iter().filter_map(|c| c.id).collect();
        let linked = self
            .get_typed_memories_by_commitment_ids(&commitment_ids)
            .await?;
        let mut linked_commitment_ids = std::collections::HashSet::new();
        for item in linked {
            if let Some(cid) = item.commitment_id {
                linked_commitment_ids.insert(cid);
            }
            merge_hybrid_candidate(
                &mut gathered,
                query.user_id,
                &query.exclude_kinds,
                item,
                0.0,
                MemoryCandidateSource::Commitment,
            );
        }
        for commitment in &commitments {
            let Some(cid) = commitment.id else {
                continue;
            };
            if linked_commitment_ids.contains(&cid) {
                continue;
            }
            // Ephemeral MemoryItem keyed by negated commitment id to avoid
            // colliding with typed_memories primary keys in the gather map.
            let item = crate::MemoryItem {
                id: Some(0_i64.wrapping_sub(cid)),
                scope: crate::MemoryScope::Character,
                character_id: commitment.character_id.clone(),
                user_id: commitment.user_id.clone(),
                kind: crate::MemoryKind::Commitment,
                title: commitment.title.clone(),
                content: commitment.description.clone(),
                source: crate::MemorySource::Conversation,
                source_ref: Some(format!("commitment:{cid}")),
                confidence: crate::MemoryConfidence::new(0.9),
                salience: crate::MemorySalience::new(0.8),
                affect: crate::AffectAnnotation::default(),
                relationship_impact: 0.0,
                access_count: 0,
                last_accessed_at: None,
                created_at: commitment.created_at,
                updated_at: commitment.updated_at,
                valid_from: None,
                valid_until: commitment.due_at,
                status: crate::MemoryStatus::Active,
                supersedes_id: None,
                pinned: false,
                faded_at: None,
                commitment_id: Some(cid),
            };
            merge_hybrid_candidate(
                &mut gathered,
                query.user_id,
                &query.exclude_kinds,
                item,
                0.0,
                MemoryCandidateSource::Commitment,
            );
        }

        // Limited recent fallback for memories not already gathered.
        if query.recent_fallback_limit > 0 {
            let recent_candidates = self
                .list_recallable_typed_memories(
                    query.character_id,
                    query.user_id,
                    query.recent_fallback_limit.saturating_mul(2).max(pool),
                )
                .await?;
            let mut recent_added = 0usize;
            for item in recent_candidates {
                if recent_added >= query.recent_fallback_limit {
                    break;
                }
                let Some(id) = item.id else {
                    continue;
                };
                if gathered.contains_key(&id) {
                    continue;
                }
                merge_hybrid_candidate(
                    &mut gathered,
                    query.user_id,
                    &query.exclude_kinds,
                    item,
                    0.0,
                    MemoryCandidateSource::Recent,
                );
                recent_added = recent_added.saturating_add(1);
            }
        }

        Ok(gathered.into_values().collect())
    }

    /// Legacy vector-only search over `active` memories (tests / internal).
    #[cfg(test)]
    pub(crate) async fn search_typed_memories(
        &self,
        query_embedding: &[f32],
        character_id: &str,
        model_name: &str,
        limit: usize,
        similarity_threshold: f32,
    ) -> Result<Vec<(crate::MemoryItem, f32)>, EneMemoryError> {
        self.search_typed_memories_vector(
            query_embedding,
            character_id,
            model_name,
            None,
            &[crate::MemoryStatus::Active.as_str()],
            limit,
            similarity_threshold,
        )
        .await
    }

    /// Vector similarity search with configurable recallable statuses.
    ///
    /// Uses the `vec0` ANN index (`vec_memory_embeddings`) for candidate
    /// retrieval, then joins back to `typed_memories` for status and
    /// user-visibility filtering (#304).
    pub(crate) async fn search_typed_memories_vector(
        &self,
        query_embedding: &[f32],
        character_id: &str,
        model_name: &str,
        user_id: Option<&str>,
        statuses: &[&str],
        limit: usize,
        similarity_threshold: f32,
    ) -> Result<Vec<(crate::MemoryItem, f32)>, EneMemoryError> {
        use sea_orm::{DbBackend, FromQueryResult, Statement};

        #[derive(Debug, FromQueryResult)]
        struct VecSearchRow {
            id: i64,
            scope: String,
            character_id: String,
            user_id: String,
            kind: String,
            title: String,
            content: String,
            source: String,
            source_ref: Option<String>,
            confidence: f32,
            salience: f32,
            affective_valence: f32,
            affective_arousal: f32,
            relationship_impact: f32,
            access_count: i64,
            last_accessed_at: Option<DateTime<Utc>>,
            created_at: DateTime<Utc>,
            updated_at: DateTime<Utc>,
            valid_from: Option<DateTime<Utc>>,
            valid_until: Option<DateTime<Utc>>,
            status: String,
            supersedes_id: Option<i64>,
            pinned: i32,
            faded_at: Option<DateTime<Utc>>,
            commitment_id: Option<i64>,
            similarity: f64,
        }

        validate_embedding(query_embedding, self.embedding_dim)?;

        let query_bytes = embedding_to_bytes(query_embedding);
        let max_distance = 1.0_f64 - f64::from(similarity_threshold);
        // Over-fetch from the ANN index to survive post-KNN status/user
        // filtering. Factor of 4 matches the tool-search heuristic.
        let knn_k = (limit as u64).saturating_mul(4).max(limit as u64).max(1);

        let status_placeholders: Vec<&str> = statuses.iter().map(|_| "?").collect();
        let status_list = status_placeholders.join(", ");

        let user_filter = if user_id.is_some() {
            "AND (tm.user_id = ? OR tm.user_id = '')"
        } else {
            ""
        };

        let sql = format!(
            "WITH knn AS ( \
                 SELECT memory_embedding_id, distance \
                 FROM vec_memory_embeddings \
                 WHERE embedding MATCH ? \
                   AND k = ? \
                   AND character_id = ? \
                   AND model_name = ? \
                   AND field = 'content' \
                   AND distance <= ? \
             ) \
             SELECT \
                 tm.id, tm.scope, tm.character_id, tm.user_id, tm.kind, \
                 tm.title, tm.content, tm.source, tm.source_ref, \
                 tm.confidence, tm.salience, \
                 tm.affective_valence, tm.affective_arousal, \
                 tm.relationship_impact, tm.access_count, \
                 tm.last_accessed_at, tm.created_at, tm.updated_at, \
                 tm.valid_from, tm.valid_until, tm.status, \
                 tm.supersedes_id, tm.pinned, tm.faded_at, tm.commitment_id, \
                 1.0 - knn.distance AS similarity \
             FROM knn \
             INNER JOIN memory_embeddings me ON me.id = knn.memory_embedding_id \
             INNER JOIN typed_memories tm ON tm.id = me.memory_item_id \
             WHERE tm.character_id = ? \
               AND tm.status IN ({status_list}) \
               {user_filter} \
             ORDER BY knn.distance ASC \
             LIMIT ?"
        );

        let mut values: Vec<sea_orm::Value> = vec![
            sea_orm::Value::from(query_bytes),
            sea_orm::Value::from(knn_k),
            sea_orm::Value::from(character_id.to_string()),
            sea_orm::Value::from(model_name.to_string()),
            sea_orm::Value::from(max_distance),
            sea_orm::Value::from(character_id.to_string()),
        ];
        for s in statuses {
            values.push(sea_orm::Value::from((*s).to_string()));
        }
        if let Some(uid) = user_id {
            values.push(sea_orm::Value::from(uid.to_string()));
        }
        values.push(sea_orm::Value::from(limit as u64));

        let rows = self
            .db
            .query_all_raw(Statement::from_sql_and_values(
                DbBackend::Sqlite,
                &sql,
                values,
            ))
            .await?;

        rows.iter()
            .map(|row| {
                let r = VecSearchRow::from_query_result(row, "")?;
                Ok((
                    crate::MemoryItem {
                        id: Some(r.id),
                        scope: crate::MemoryScope::from_db_str(&r.scope),
                        character_id: r.character_id,
                        user_id: r.user_id,
                        kind: crate::MemoryKind::from_db_str(&r.kind),
                        title: r.title,
                        content: r.content,
                        source: crate::MemorySource::from_db_str(&r.source),
                        source_ref: r.source_ref,
                        confidence: crate::MemoryConfidence::new(r.confidence),
                        salience: crate::MemorySalience::new(r.salience),
                        affect: crate::AffectAnnotation {
                            valence: r.affective_valence,
                            arousal: r.affective_arousal,
                        },
                        relationship_impact: r.relationship_impact,
                        access_count: r.access_count,
                        last_accessed_at: r.last_accessed_at,
                        created_at: r.created_at,
                        updated_at: r.updated_at,
                        valid_from: r.valid_from,
                        valid_until: r.valid_until,
                        status: crate::MemoryStatus::from_db_str(&r.status),
                        supersedes_id: r.supersedes_id,
                        pinned: r.pinned != 0,
                        faded_at: r.faded_at,
                        commitment_id: r.commitment_id,
                    },
                    r.similarity as f32,
                ))
            })
            .collect()
    }

    /// Brute-force vector search (full-row scan) — retained for test
    /// comparison against the ANN-indexed path (#304).
    #[cfg(test)]
    pub(crate) async fn search_typed_memories_vector_brute_force(
        &self,
        query_embedding: &[f32],
        character_id: &str,
        model_name: &str,
        user_id: Option<&str>,
        statuses: &[&str],
        limit: usize,
        similarity_threshold: f32,
    ) -> Result<Vec<(crate::MemoryItem, f32)>, EneMemoryError> {
        use super::{EmbeddingCol, cosine_similarity_expr, cosine_similarity_filter};
        use sea_orm::{FromQueryResult, QueryOrder, QuerySelect};

        #[derive(Debug, FromQueryResult)]
        struct SearchMemoryRow {
            id: i64,
            scope: String,
            character_id: String,
            user_id: String,
            kind: String,
            title: String,
            content: String,
            source: String,
            source_ref: Option<String>,
            confidence: f32,
            salience: f32,
            affective_valence: f32,
            affective_arousal: f32,
            relationship_impact: f32,
            access_count: i64,
            last_accessed_at: Option<DateTime<Utc>>,
            created_at: DateTime<Utc>,
            updated_at: DateTime<Utc>,
            valid_from: Option<DateTime<Utc>>,
            valid_until: Option<DateTime<Utc>>,
            status: String,
            supersedes_id: Option<i64>,
            pinned: i32,
            faded_at: Option<DateTime<Utc>>,
            commitment_id: Option<i64>,
            similarity: f64,
        }

        let query_bytes = embedding_to_bytes(query_embedding);
        let similarity_expr = cosine_similarity_expr(EmbeddingCol::Qualified, &query_bytes);

        let threshold_val = f64::from(similarity_threshold);
        let limit_val = limit as u64;

        let mut select = entities::memory_embeddings::Entity::find()
            .inner_join(entities::typed_memories::Entity)
            .select_only()
            .column(entities::typed_memories::Column::Id)
            .column(entities::typed_memories::Column::Scope)
            .column(entities::typed_memories::Column::CharacterId)
            .column(entities::typed_memories::Column::UserId)
            .column(entities::typed_memories::Column::Kind)
            .column(entities::typed_memories::Column::Title)
            .column(entities::typed_memories::Column::Content)
            .column(entities::typed_memories::Column::Source)
            .column(entities::typed_memories::Column::SourceRef)
            .column(entities::typed_memories::Column::Confidence)
            .column(entities::typed_memories::Column::Salience)
            .column(entities::typed_memories::Column::AffectiveValence)
            .column(entities::typed_memories::Column::AffectiveArousal)
            .column(entities::typed_memories::Column::RelationshipImpact)
            .column(entities::typed_memories::Column::AccessCount)
            .column(entities::typed_memories::Column::LastAccessedAt)
            .column(entities::typed_memories::Column::CreatedAt)
            .column(entities::typed_memories::Column::UpdatedAt)
            .column(entities::typed_memories::Column::ValidFrom)
            .column(entities::typed_memories::Column::ValidUntil)
            .column(entities::typed_memories::Column::Status)
            .column(entities::typed_memories::Column::SupersedesId)
            .column(entities::typed_memories::Column::Pinned)
            .column(entities::typed_memories::Column::FadedAt)
            .column(entities::typed_memories::Column::CommitmentId)
            .expr_as(similarity_expr, "similarity")
            .filter(entities::typed_memories::Column::CharacterId.eq(character_id))
            .filter(entities::typed_memories::Column::Status.is_in(statuses.to_vec()))
            .filter(entities::memory_embeddings::Column::ModelName.eq(model_name))
            .filter(entities::memory_embeddings::Column::Field.eq("content"))
            .filter(cosine_similarity_filter(
                EmbeddingCol::Qualified,
                &query_bytes,
                threshold_val,
            ))
            .order_by_desc(Expr::col("similarity"))
            .limit(limit_val);

        if let Some(uid) = user_id {
            select = select.filter(user_visibility_condition(uid));
        }

        let rows = select.into_model::<SearchMemoryRow>().all(&self.db).await?;

        rows.into_iter()
            .map(|row| {
                Ok((
                    crate::MemoryItem {
                        id: Some(row.id),
                        scope: crate::MemoryScope::from_db_str(&row.scope),
                        character_id: row.character_id,
                        user_id: row.user_id,
                        kind: crate::MemoryKind::from_db_str(&row.kind),
                        title: row.title,
                        content: row.content,
                        source: crate::MemorySource::from_db_str(&row.source),
                        source_ref: row.source_ref,
                        confidence: crate::MemoryConfidence::new(row.confidence),
                        salience: crate::MemorySalience::new(row.salience),
                        affect: crate::AffectAnnotation {
                            valence: row.affective_valence,
                            arousal: row.affective_arousal,
                        },
                        relationship_impact: row.relationship_impact,
                        access_count: row.access_count,
                        last_accessed_at: row.last_accessed_at,
                        created_at: row.created_at,
                        updated_at: row.updated_at,
                        valid_from: row.valid_from,
                        valid_until: row.valid_until,
                        status: crate::MemoryStatus::from_db_str(&row.status),
                        supersedes_id: row.supersedes_id,
                        pinned: row.pinned != 0,
                        faded_at: row.faded_at,
                        commitment_id: row.commitment_id,
                    },
                    row.similarity as f32,
                ))
            })
            .collect()
    }

    /// List typed memories eligible for hybrid recall (`active`, `faded`, `disputed`).
    pub async fn list_recallable_typed_memories(
        &self,
        character_id: &str,
        user_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<crate::MemoryItem>, EneMemoryError> {
        use sea_orm::{EntityTrait, QueryFilter, QueryOrder, QuerySelect};

        let mut query = entities::typed_memories::Entity::find()
            .filter(entities::typed_memories::Column::CharacterId.eq(character_id))
            .filter(entities::typed_memories::Column::Status.is_in(RECALLABLE_STATUSES));

        if let Some(uid) = user_id {
            query = query.filter(user_visibility_condition(uid));
        }

        let models = query
            .order_by_desc(entities::typed_memories::Column::UpdatedAt)
            .limit(limit as u64)
            .all(&self.db)
            .await?;

        models
            .into_iter()
            .map(model_to_memory_item)
            .collect::<Result<Vec<_>, _>>()
    }

    /// Fetch typed memories linked to commitment ledger rows (#124).
    async fn get_typed_memories_by_commitment_ids(
        &self,
        commitment_ids: &[i64],
    ) -> Result<Vec<crate::MemoryItem>, EneMemoryError> {
        use sea_orm::{EntityTrait, QueryFilter};

        if commitment_ids.is_empty() {
            return Ok(Vec::new());
        }

        let models = entities::typed_memories::Entity::find()
            .filter(entities::typed_memories::Column::CommitmentId.is_in(commitment_ids.to_vec()))
            .all(&self.db)
            .await?;

        models
            .into_iter()
            .map(model_to_memory_item)
            .collect::<Result<Vec<_>, _>>()
    }

    /// List recallable typed memories whose title or content matches query tokens.
    async fn list_lexical_typed_memory_candidates(
        &self,
        query_text: &str,
        character_id: &str,
        user_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<crate::MemoryItem>, EneMemoryError> {
        use crate::search::tokenize;
        use sea_orm::{Condition, EntityTrait, QueryFilter, QueryOrder, QuerySelect};

        let tokens: Vec<String> = tokenize(query_text).into_iter().collect();
        if tokens.is_empty() {
            return Ok(Vec::new());
        }

        let mut lexical_match = Condition::any();
        for token in tokens {
            let pattern = format!("%{token}%");
            lexical_match = lexical_match
                .add(entities::typed_memories::Column::Title.like(&pattern))
                .add(entities::typed_memories::Column::Content.like(&pattern));
        }

        let mut query = entities::typed_memories::Entity::find()
            .filter(entities::typed_memories::Column::CharacterId.eq(character_id))
            .filter(entities::typed_memories::Column::Status.is_in(RECALLABLE_STATUSES))
            .filter(lexical_match);

        if let Some(uid) = user_id {
            query = query.filter(user_visibility_condition(uid));
        }

        let models = query
            .order_by_desc(entities::typed_memories::Column::UpdatedAt)
            .limit(limit as u64)
            .all(&self.db)
            .await?;

        models
            .into_iter()
            .map(model_to_memory_item)
            .collect::<Result<Vec<_>, _>>()
    }

    /// Atomically insert a replacement memory and mark the prior row superseded.
    ///
    /// The new row's `supersedes_id` is set to `superseded_id` (predecessor link).
    /// The old row is transitioned to [`crate::MemoryStatus::Superseded`] with
    /// `supersedes_id` cleared. Only rows in `Active`, `Faded`, or `Disputed`
    /// status may be superseded.
    pub async fn supersede_typed_memory(
        &self,
        new_item: &crate::NewMemoryItem,
        superseded_id: i64,
    ) -> Result<i64, EneMemoryError> {
        use sea_orm::{ActiveModelTrait, ActiveValue::Set, EntityTrait, TransactionTrait};

        let txn = self.db.begin().await?;

        let old_model = entities::typed_memories::Entity::find_by_id(superseded_id)
            .one(&txn)
            .await?
            .ok_or_else(|| {
                EneMemoryError::Other(format!("superseded memory id={superseded_id} not found"))
            })?;

        let old_status = crate::MemoryStatus::from_db_str(&old_model.status);
        if !is_supersedeable_status(old_status) {
            return Err(EneMemoryError::Other(format!(
                "memory id={superseded_id} cannot be superseded (status={})",
                old_model.status
            )));
        }

        let now = Utc::now();
        let mut insert_item = new_item.clone();
        insert_item.supersedes_id = Some(superseded_id);

        let active = entities::typed_memories::ActiveModel {
            scope: Set(insert_item.scope.as_str().to_string()),
            character_id: Set(insert_item.character_id.clone()),
            user_id: Set(insert_item.user_id.clone()),
            kind: Set(insert_item.kind.as_str().to_string()),
            title: Set(insert_item.title.clone()),
            content: Set(insert_item.content.clone()),
            source: Set(insert_item.source.as_str().to_string()),
            source_ref: Set(insert_item.source_ref.clone()),
            confidence: Set(insert_item.confidence.get()),
            salience: Set(insert_item.salience.get()),
            affective_valence: Set(insert_item.affect.valence),
            affective_arousal: Set(insert_item.affect.arousal),
            relationship_impact: Set(insert_item.relationship_impact),
            access_count: Set(0),
            last_accessed_at: Set(None),
            created_at: Set(now),
            updated_at: Set(now),
            valid_from: Set(insert_item.valid_from),
            valid_until: Set(insert_item.valid_until),
            status: Set(insert_item.status.as_str().to_string()),
            supersedes_id: Set(insert_item.supersedes_id),
            pinned: Set(i32::from(insert_item.pinned)),
            ..Default::default()
        };
        let inserted = active.insert(&txn).await?;
        let new_id = inserted.id;

        let mut old_active: entities::typed_memories::ActiveModel = old_model.into();
        old_active.status = Set(crate::MemoryStatus::Superseded.as_str().to_string());
        old_active.supersedes_id = Set(None);
        old_active.updated_at = Set(now);
        old_active.update(&txn).await?;

        txn.commit().await?;
        Ok(new_id)
    }

    /// Bump the access count and last-accessed timestamp for a typed memory.
    pub async fn bump_typed_memory_access(&self, id: i64) -> Result<bool, EneMemoryError> {
        use sea_orm::ExprTrait;

        let now = Utc::now();
        let result = entities::typed_memories::Entity::update_many()
            .col_expr(
                entities::typed_memories::Column::AccessCount,
                Expr::col(entities::typed_memories::Column::AccessCount).add(1),
            )
            .col_expr(
                entities::typed_memories::Column::LastAccessedAt,
                Expr::value(now),
            )
            .filter(entities::typed_memories::Column::Id.eq(id))
            .exec(&self.db)
            .await?;
        Ok(result.rows_affected > 0)
    }

    /// Transition a typed memory with lifecycle edge validation (#76).
    pub async fn set_memory_status(
        &self,
        id: i64,
        new_status: crate::MemoryStatus,
    ) -> Result<bool, EneMemoryError> {
        use sea_orm::{ActiveModelTrait, ActiveValue::Set, EntityTrait};

        let maybe_model = entities::typed_memories::Entity::find_by_id(id)
            .one(&self.db)
            .await?;

        let Some(model) = maybe_model else {
            return Ok(false);
        };

        let current = crate::MemoryStatus::from_db_str(&model.status);
        if let Err(invalid) = crate::forgetting::validate_transition(current, new_status) {
            return Err(EneMemoryError::InvalidTransition {
                from: invalid.from,
                to: invalid.to,
            });
        }

        let item = model_to_memory_item(model.clone())?;
        let now = Utc::now();
        let mut active: entities::typed_memories::ActiveModel = model.into();
        active.status = Set(new_status.as_str().to_string());
        active.updated_at = Set(now);
        if current == crate::MemoryStatus::Active && new_status == crate::MemoryStatus::Faded {
            active.faded_at = Set(Some(crate::forgetting::active_decay_anchor(&item)));
        }
        active.update(&self.db).await?;
        Ok(true)
    }

    /// User-driven restore to [`MemoryStatus::Active`] (journal/CLI UX).
    pub async fn user_restore_typed_memory(&self, id: i64) -> Result<bool, EneMemoryError> {
        use sea_orm::{ActiveModelTrait, ActiveValue::Set, EntityTrait};

        let maybe_model = entities::typed_memories::Entity::find_by_id(id)
            .one(&self.db)
            .await?;

        let Some(model) = maybe_model else {
            return Ok(false);
        };

        let current = crate::MemoryStatus::from_db_str(&model.status);
        if let Err(invalid) = crate::forgetting::validate_user_restore(current) {
            return Err(EneMemoryError::InvalidTransition {
                from: invalid.from,
                to: invalid.to,
            });
        }

        let now = Utc::now();
        let mut active: entities::typed_memories::ActiveModel = model.into();
        active.status = Set(crate::MemoryStatus::Active.as_str().to_string());
        active.faded_at = Set(None);
        active.updated_at = Set(now);
        active.update(&self.db).await?;
        Ok(true)
    }

    /// User-driven forget (`Active` → `UserDeleted`).
    pub async fn user_forget_typed_memory(&self, id: i64) -> Result<bool, EneMemoryError> {
        self.set_memory_status(id, crate::MemoryStatus::UserDeleted)
            .await
    }

    /// List typed memories for the memory journal with user/scope and status filters.
    pub async fn list_journal_memories(
        &self,
        options: &crate::MemoryJournalListOptions<'_>,
    ) -> Result<Vec<crate::MemoryItem>, EneMemoryError> {
        use sea_orm::{EntityTrait, QueryFilter, QueryOrder, QuerySelect};

        let mut allowed_statuses = RECALLABLE_STATUSES.to_vec();
        if options.include_archived {
            allowed_statuses.push(crate::MemoryStatus::Archived.as_str());
        }
        if options.include_superseded {
            allowed_statuses.push(crate::MemoryStatus::Superseded.as_str());
        }
        if options.include_user_deleted {
            allowed_statuses.push(crate::MemoryStatus::UserDeleted.as_str());
        }

        let mut query = entities::typed_memories::Entity::find()
            .filter(entities::typed_memories::Column::CharacterId.eq(options.character_id))
            .filter(entities::typed_memories::Column::Status.is_in(allowed_statuses));

        if let Some(uid) = options.user_id {
            query = query.filter(user_visibility_condition(uid));
        }

        if let Some(kind) = options.kind {
            query = query.filter(entities::typed_memories::Column::Kind.eq(kind.as_str()));
        }

        let models = query
            .order_by_desc(entities::typed_memories::Column::UpdatedAt)
            .limit(options.limit as u64)
            .offset(options.offset as u64)
            .all(&self.db)
            .await?;

        models
            .into_iter()
            .map(model_to_memory_item)
            .collect::<Result<Vec<_>, _>>()
    }

    /// Set whether a typed memory is pinned (exempt from natural decay).
    pub async fn pin_typed_memory(&self, id: i64, pinned: bool) -> Result<bool, EneMemoryError> {
        use sea_orm::{ActiveModelTrait, ActiveValue::Set, EntityTrait};

        let maybe_model = entities::typed_memories::Entity::find_by_id(id)
            .one(&self.db)
            .await?;

        let Some(model) = maybe_model else {
            return Ok(false);
        };

        let now = Utc::now();
        let mut active: entities::typed_memories::ActiveModel = model.into();
        active.pinned = Set(i32::from(pinned));
        active.updated_at = Set(now);
        active.update(&self.db).await?;
        Ok(true)
    }

    /// List typed memories eligible for natural decay processing.
    pub async fn list_memories_for_decay(
        &self,
        character_id: &str,
        user_id: Option<&str>,
        statuses: &[crate::MemoryStatus],
        limit: usize,
    ) -> Result<Vec<crate::MemoryItem>, EneMemoryError> {
        use sea_orm::{EntityTrait, QueryFilter, QueryOrder, QuerySelect};

        if statuses.is_empty() {
            return Ok(vec![]);
        }

        let status_strs: Vec<&str> = statuses.iter().map(|s| s.as_str()).collect();
        let mut query = entities::typed_memories::Entity::find()
            .filter(entities::typed_memories::Column::CharacterId.eq(character_id))
            .filter(entities::typed_memories::Column::Status.is_in(status_strs));

        if let Some(uid) = user_id {
            query = query.filter(user_visibility_condition(uid));
        }

        let models = query
            .order_by_asc(entities::typed_memories::Column::UpdatedAt)
            .limit(limit as u64)
            .all(&self.db)
            .await?;

        models
            .into_iter()
            .map(model_to_memory_item)
            .collect::<Result<Vec<_>, _>>()
    }

    /// Apply natural decay transitions for recallable memories in a scope.
    ///
    /// Uses a single SQL `UPDATE` per transition edge (active→faded, faded→archived)
    /// so the pass scales to the full table without a `BATCH_LIMIT` (#350).
    pub async fn apply_natural_decay_batch(
        &self,
        character_id: &str,
        user_id: Option<&str>,
        now: DateTime<Utc>,
        half_life_days: f64,
        fade_threshold: f32,
        archive_threshold: f32,
    ) -> Result<NaturalDecayReport, EneMemoryError> {
        use sea_orm::ConnectionTrait;

        if half_life_days <= 0.0 || half_life_days.is_nan() {
            return Ok(NaturalDecayReport::default());
        }

        let now_text = now.format("%Y-%m-%d %H:%M:%S").to_string();
        let cid = escape_sql_literal(character_id);
        let user_clause = match user_id {
            Some(uid) => format!(
                " AND (user_id = '{}' OR user_id = '')",
                escape_sql_literal(uid)
            ),
            None => String::new(),
        };

        let emotional = "CASE WHEN sqrt(affective_valence * affective_valence + affective_arousal * affective_arousal) / 2.83 > 1.0 THEN 1.0 ELSE sqrt(affective_valence * affective_valence + affective_arousal * affective_arousal) / 2.83 END";

        let decay_active = format!(
            "exp(-0.6931471805599453 / {half_life_days} * (julianday('{now_text}') - julianday(updated_at))) * (0.5 * salience + 0.5) * (0.5 * confidence + 0.5) * (0.3 * {emotional} + 0.7)"
        );
        let decay_faded = format!(
            "exp(-0.6931471805599453 / {half_life_days} * (julianday('{now_text}') - julianday(coalesce(faded_at, created_at)))) * (0.5 * salience + 0.5) * (0.5 * confidence + 0.5) * (0.3 * {emotional} + 0.7)"
        );

        let fade_to_faded = format!(
            "UPDATE typed_memories SET status = 'faded', faded_at = '{now_text}', updated_at = '{now_text}' WHERE character_id = '{cid}' AND status = 'active' AND pinned = 0{user_clause} AND ({decay_active}) < {fade_threshold}"
        );
        let faded_to_archived = format!(
            "UPDATE typed_memories SET status = 'archived', updated_at = '{now_text}' WHERE character_id = '{cid}' AND status = 'faded' AND pinned = 0{user_clause} AND ({decay_faded}) < {archive_threshold}"
        );

        self.db.execute_unprepared(&fade_to_faded).await?;
        let faded_count = query_sqlite_changes(&self.db).await?;

        self.db.execute_unprepared(&faded_to_archived).await?;
        let archived_count = query_sqlite_changes(&self.db).await?;

        Ok(NaturalDecayReport {
            faded_count,
            archived_count,
        })
    }

    /// Backdate typed memory timestamps for integration tests (#76).
    #[doc(hidden)]
    pub async fn test_backdate_typed_memory(
        &self,
        id: i64,
        days_ago: i64,
    ) -> Result<bool, EneMemoryError> {
        use sea_orm::{ActiveModelTrait, ActiveValue::Set, EntityTrait};

        let maybe_model = entities::typed_memories::Entity::find_by_id(id)
            .one(&self.db)
            .await?;
        let Some(model) = maybe_model else {
            return Ok(false);
        };
        let anchor = Utc::now()
            .checked_sub_signed(chrono::Duration::days(days_ago))
            .unwrap_or_else(Utc::now);
        let mut active: entities::typed_memories::ActiveModel = model.into();
        active.created_at = Set(anchor);
        active.updated_at = Set(anchor);
        active.last_accessed_at = Set(None);
        active.update(&self.db).await?;
        Ok(true)
    }

    /// Store a content embedding for a typed memory item.
    pub async fn upsert_memory_embedding(
        &self,
        memory_item_id: i64,
        model_name: &str,
        field: &str,
        embedding: &[f32],
    ) -> Result<(), EneMemoryError> {
        use sea_orm::sea_query::OnConflict;
        use sea_orm::{ActiveValue::Set, EntityTrait};

        validate_embedding(embedding, self.embedding_dim)?;

        let now = Utc::now();
        let embedding_bytes = embedding_to_bytes(embedding);

        let active = entities::memory_embeddings::ActiveModel {
            memory_item_id: Set(memory_item_id),
            model_name: Set(model_name.to_string()),
            field: Set(field.to_string()),
            embedding: Set(embedding_bytes),
            created_at: Set(now),
            ..Default::default()
        };

        entities::memory_embeddings::Entity::insert(active)
            .on_conflict(
                OnConflict::columns([
                    entities::memory_embeddings::Column::MemoryItemId,
                    entities::memory_embeddings::Column::ModelName,
                    entities::memory_embeddings::Column::Field,
                ])
                .update_column(entities::memory_embeddings::Column::Embedding)
                .to_owned(),
            )
            .exec(&self.db)
            .await?;

        Ok(())
    }

    // ── Memory Spans ────────────────────────────────────────────────────────

    /// Distinct session IDs that have conversation logs for a character card.
    pub async fn list_session_ids_for_card(
        &self,
        card_name: &str,
    ) -> Result<Vec<String>, EneMemoryError> {
        list_session_ids_for_card_on_conn(&self.db, card_name).await
    }

    /// Returns true when a memory span already exists for the session turn.
    pub async fn memory_span_exists(
        &self,
        session_id: &str,
        turn_start: i32,
    ) -> Result<bool, EneMemoryError> {
        use sea_orm::PaginatorTrait;

        let count = entities::memory_spans::Entity::find()
            .filter(entities::memory_spans::Column::SessionId.eq(session_id))
            .filter(entities::memory_spans::Column::TurnStart.eq(turn_start))
            .count(&self.db)
            .await?;
        Ok(count > 0)
    }

    /// Insert a memory span row.
    pub async fn insert_memory_span(&self, span: &NewMemorySpan) -> Result<i64, EneMemoryError> {
        use sea_orm::ActiveModelTrait;
        use sea_orm::ActiveValue::Set;

        let active = entities::memory_spans::ActiveModel {
            session_id: Set(span.session_id.clone()),
            turn_start: Set(span.turn_start),
            turn_end: Set(span.turn_end),
            raw_excerpt: Set(span.raw_excerpt.clone()),
            compressed_summary: Set(span.compressed_summary.clone()),
            compression_level: Set(span.compression_level),
            ..Default::default()
        };
        let res = active.insert(&self.db).await?;
        Ok(res.id)
    }

    /// List memory spans for a session ordered by turn start.
    pub async fn list_memory_spans_by_session(
        &self,
        session_id: &str,
    ) -> Result<Vec<NewMemorySpan>, EneMemoryError> {
        use sea_orm::QueryOrder;

        let rows = entities::memory_spans::Entity::find()
            .filter(entities::memory_spans::Column::SessionId.eq(session_id))
            .order_by_asc(entities::memory_spans::Column::TurnStart)
            .all(&self.db)
            .await?;

        Ok(rows
            .into_iter()
            .map(|r| NewMemorySpan {
                session_id: r.session_id,
                turn_start: r.turn_start,
                turn_end: r.turn_end,
                raw_excerpt: r.raw_excerpt,
                compressed_summary: r.compressed_summary,
                compression_level: r.compression_level,
            })
            .collect())
    }

    /// List memory spans for a session filtered by compression level.
    pub async fn list_memory_spans_by_session_and_level(
        &self,
        session_id: &str,
        compression_level: i32,
    ) -> Result<Vec<NewMemorySpan>, EneMemoryError> {
        use sea_orm::QueryOrder;

        let rows = entities::memory_spans::Entity::find()
            .filter(entities::memory_spans::Column::SessionId.eq(session_id))
            .filter(entities::memory_spans::Column::CompressionLevel.eq(compression_level))
            .order_by_asc(entities::memory_spans::Column::TurnStart)
            .all(&self.db)
            .await?;

        Ok(rows
            .into_iter()
            .map(|r| NewMemorySpan {
                session_id: r.session_id,
                turn_start: r.turn_start,
                turn_end: r.turn_end,
                raw_excerpt: r.raw_excerpt,
                compressed_summary: r.compressed_summary,
                compression_level: r.compression_level,
            })
            .collect())
    }

    /// Return the latest scene-level compressed summary for a session.
    pub async fn get_active_scene_summary(
        &self,
        session_id: &str,
    ) -> Result<Option<ActiveSceneSummaryRow>, EneMemoryError> {
        use sea_orm::QueryOrder;

        let row = entities::memory_spans::Entity::find()
            .filter(entities::memory_spans::Column::SessionId.eq(session_id))
            .filter(entities::memory_spans::Column::CompressedSummary.is_not_null())
            .order_by_desc(entities::memory_spans::Column::CompressionLevel)
            .order_by_desc(entities::memory_spans::Column::TurnEnd)
            .one(&self.db)
            .await?;

        Ok(row.and_then(|r| {
            let summary = r.compressed_summary?;
            if summary.trim().is_empty() {
                return None;
            }
            Some(ActiveSceneSummaryRow {
                span_id: r.id,
                summary,
                compression_level: r.compression_level,
            })
        }))
    }

    /// Update the compressed summary for an existing span.
    pub async fn update_span_summary(
        &self,
        span_id: i64,
        summary: &str,
    ) -> Result<(), EneMemoryError> {
        use sea_orm::{ActiveModelTrait, ActiveValue::Set};

        let mut active: entities::memory_spans::ActiveModel = entities::memory_spans::ActiveModel {
            id: Set(span_id),
            ..Default::default()
        };
        active.compressed_summary = Set(Some(summary.to_string()));
        active.update(&self.db).await?;
        Ok(())
    }
}
