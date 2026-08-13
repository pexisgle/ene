//! Commitment ledger queries.

use super::{EneMemoryError, MemoryStore};
use crate::entities;
use chrono::{DateTime, Utc};
use sea_orm::sea_query::Expr;
use sea_orm::{ColumnTrait, EntityTrait};

/// Convert a commitment model row to a [`crate::Commitment`].
#[expect(
    clippy::unnecessary_wraps,
    reason = "store helper signature returns Result for uniform error propagation"
)]
fn model_to_commitment(
    m: entities::commitments::Model,
) -> Result<crate::Commitment, EneMemoryError> {
    Ok(crate::Commitment {
        id: Some(m.id),
        character_id: m.character_id,
        user_id: m.user_id,
        title: m.title,
        description: m.description,
        status: crate::CommitmentStatus::from_db_str(&m.status),
        due_at: m.due_at,
        due_label: m.due_label,
        created_at: m.created_at,
        updated_at: m.updated_at,
        completed_at: m.completed_at,
    })
}

impl MemoryStore {
    /// Insert a new commitment row and return its assigned ID.
    pub async fn insert_commitment(
        &self,
        item: &crate::NewCommitment,
    ) -> Result<i64, EneMemoryError> {
        use sea_orm::ActiveModelTrait;
        use sea_orm::ActiveValue::Set;

        let now = Utc::now();
        let active = entities::commitments::ActiveModel {
            character_id: Set(item.character_id.clone()),
            user_id: Set(item.user_id.clone()),
            title: Set(item.title.clone()),
            description: Set(item.description.clone()),
            status: Set(item.status.as_str().to_string()),
            due_at: Set(item.due_at),
            due_label: Set(item.due_label.clone()),
            created_at: Set(now),
            updated_at: Set(now),
            completed_at: Set(None),
            ..Default::default()
        };
        let res = active.insert(&self.db).await?;
        Ok(res.id)
    }

    /// Retrieve a commitment by its ID.
    pub async fn get_commitment(
        &self,
        id: i64,
    ) -> Result<Option<crate::Commitment>, EneMemoryError> {
        let maybe_model = entities::commitments::Entity::find_by_id(id)
            .one(&self.db)
            .await?;
        match maybe_model {
            Some(m) => model_to_commitment(m).map(Some),
            None => Ok(None),
        }
    }

    /// List active commitments for a character, optionally scoped to a user.
    ///
    /// Results are ordered by `due_at` ascending (nulls last), then `created_at`
    /// descending so undated follow-ups still surface recently.
    pub async fn list_active_commitments(
        &self,
        character_id: &str,
        user_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<crate::Commitment>, EneMemoryError> {
        use sea_orm::{EntityTrait, QueryFilter, QueryOrder, QuerySelect};

        let mut query = entities::commitments::Entity::find()
            .filter(entities::commitments::Column::CharacterId.eq(character_id))
            .filter(
                entities::commitments::Column::Status.eq(crate::CommitmentStatus::Active.as_str()),
            );

        if let Some(uid) = user_id {
            query = query.filter(entities::commitments::Column::UserId.eq(uid));
        }

        // SQLite sorts NULLs first on plain ASC; order by `due_at IS NULL` so dated rows
        // surface before undated follow-ups.
        let models = query
            .order_by_asc(Expr::cust("due_at IS NULL"))
            .order_by_asc(entities::commitments::Column::DueAt)
            .order_by_desc(entities::commitments::Column::CreatedAt)
            .limit(limit as u64)
            .all(&self.db)
            .await?;

        models
            .into_iter()
            .map(model_to_commitment)
            .collect::<Result<Vec<_>, _>>()
    }

    /// List commitments for a character in every lifecycle status.
    ///
    /// Optional status filter; ordered by `created_at` descending so the most
    /// recent ledger entries surface first, with `id` descending as a
    /// deterministic tie-breaker for same-timestamp inserts.
    pub async fn list_commitments(
        &self,
        character_id: &str,
        user_id: Option<&str>,
        status: Option<crate::CommitmentStatus>,
        limit: usize,
    ) -> Result<Vec<crate::Commitment>, EneMemoryError> {
        use sea_orm::{EntityTrait, QueryFilter, QueryOrder, QuerySelect};

        let mut query = entities::commitments::Entity::find()
            .filter(entities::commitments::Column::CharacterId.eq(character_id));

        if let Some(uid) = user_id {
            query = query.filter(entities::commitments::Column::UserId.eq(uid));
        }
        if let Some(status) = status {
            query = query.filter(entities::commitments::Column::Status.eq(status.as_str()));
        }

        let models = query
            .order_by_desc(entities::commitments::Column::CreatedAt)
            .order_by_desc(entities::commitments::Column::Id)
            .limit(limit as u64)
            .all(&self.db)
            .await?;

        models
            .into_iter()
            .map(model_to_commitment)
            .collect::<Result<Vec<_>, _>>()
    }

    /// Transition an active commitment to a new lifecycle status.
    ///
    /// Returns `Ok(false)` when the row does not exist or is no longer `active`.
    pub async fn update_commitment_status(
        &self,
        id: i64,
        new_status: crate::CommitmentStatus,
    ) -> Result<bool, EneMemoryError> {
        use sea_orm::sea_query::Expr;
        use sea_orm::{EntityTrait, QueryFilter};

        let now = Utc::now();
        let mut stmt = entities::commitments::Entity::update_many()
            .col_expr(
                entities::commitments::Column::Status,
                Expr::value(new_status.as_str().to_string()),
            )
            .col_expr(entities::commitments::Column::UpdatedAt, Expr::value(now))
            .filter(entities::commitments::Column::Id.eq(id))
            .filter(
                entities::commitments::Column::Status.eq(crate::CommitmentStatus::Active.as_str()),
            );
        if new_status == crate::CommitmentStatus::Done {
            stmt = stmt.col_expr(
                entities::commitments::Column::CompletedAt,
                Expr::value(Some(now)),
            );
        }
        let result = stmt.exec(&self.db).await?;
        Ok(result.rows_affected > 0)
    }

    /// Update an active commitment's description, due label, and parsed due
    /// datetime in-place.
    ///
    /// Only succeeds when the row exists and is `Active`. Returns `Ok(false)`
    /// when the row does not exist or is no longer active.
    ///
    /// Uses a single atomic `UPDATE ... WHERE status = 'active'` to prevent
    /// TOCTOU races between the former find→check→update round trips.
    pub async fn supersede_commitment(
        &self,
        id: i64,
        description: &str,
        due_label: Option<&str>,
        due_at: Option<DateTime<Utc>>,
    ) -> Result<bool, EneMemoryError> {
        use sea_orm::sea_query::Expr;
        use sea_orm::{EntityTrait, QueryFilter};

        let now = Utc::now();
        let result = entities::commitments::Entity::update_many()
            .col_expr(
                entities::commitments::Column::Description,
                Expr::value(description.to_string()),
            )
            .col_expr(
                entities::commitments::Column::DueLabel,
                Expr::value(due_label.map(ToOwned::to_owned)),
            )
            .col_expr(entities::commitments::Column::DueAt, Expr::value(due_at))
            .col_expr(entities::commitments::Column::UpdatedAt, Expr::value(now))
            .filter(entities::commitments::Column::Id.eq(id))
            .filter(
                entities::commitments::Column::Status.eq(crate::CommitmentStatus::Active.as_str()),
            )
            .exec(&self.db)
            .await?;
        Ok(result.rows_affected > 0)
    }

    /// Mark a commitment as done.
    pub async fn complete_commitment(&self, id: i64) -> Result<bool, EneMemoryError> {
        self.update_commitment_status(id, crate::CommitmentStatus::Done)
            .await
    }

    /// Mark a commitment as cancelled.
    pub async fn cancel_commitment(&self, id: i64) -> Result<bool, EneMemoryError> {
        self.update_commitment_status(id, crate::CommitmentStatus::Cancelled)
            .await
    }

    /// Mark active commitments whose `due_at` is before `now` as stale.
    ///
    /// Returns the number of rows updated.
    pub async fn mark_stale_commitments(
        &self,
        now: DateTime<Utc>,
    ) -> Result<usize, EneMemoryError> {
        use sea_orm::{EntityTrait, QueryFilter};

        let result = entities::commitments::Entity::update_many()
            .col_expr(
                entities::commitments::Column::Status,
                sea_orm::sea_query::Expr::value(
                    crate::CommitmentStatus::Stale.as_str().to_string(),
                ),
            )
            .col_expr(
                entities::commitments::Column::UpdatedAt,
                sea_orm::sea_query::Expr::value(now),
            )
            .filter(
                entities::commitments::Column::Status.eq(crate::CommitmentStatus::Active.as_str()),
            )
            .filter(entities::commitments::Column::DueAt.is_not_null())
            .filter(entities::commitments::Column::DueAt.lt(now))
            .exec(&self.db)
            .await?;

        Ok(result.rows_affected as usize)
    }
}
