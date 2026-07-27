//! Session, conversation-log, and export/import queries.

use super::{ConversationLogEntry, EneMemoryError, MemoryStore};
use crate::entities;
use chrono::Utc;
use sea_orm::sea_query::{Expr, OnConflict};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QueryOrder, QuerySelect, TransactionTrait};
use std::sync::Arc;

fn log_row_to_message(row: entities::conversation_logs::Model) -> crate::export::ExportedMessage {
    crate::export::ExportedMessage {
        role: row.role,
        content: row.content,
        created_at: row.created_at,
    }
}

impl MemoryStore {
    // ── Conversation Logs ─────────────────────────────────────────────────────

    /// Inserts a conversation log entry.
    pub async fn insert_log(
        &self,
        session_id: &str,
        card_name: &str,
        role: &str,
        content: &str,
    ) -> Result<i64, EneMemoryError> {
        use sea_orm::ActiveModelTrait;
        use sea_orm::ActiveValue::Set;

        let now = Utc::now();
        let new_log = entities::conversation_logs::ActiveModel {
            session_id: Set(session_id.to_string()),
            card_name: Set(card_name.to_string()),
            role: Set(role.to_string()),
            content: Set(content.to_string()),
            created_at: Set(now),
            ..Default::default()
        };

        let res = new_log.insert(&self.db).await?;

        Ok(res.id)
    }

    /// Inserts a full conversation turn (user message + assistant response)
    /// as two log entries in a single transaction.
    pub async fn insert_conversation_turn(
        &self,
        session_id: &str,
        card_name: &str,
        user_message: &str,
        assistant_response: &str,
    ) -> Result<(i64, i64), EneMemoryError> {
        use sea_orm::ActiveModelTrait;
        use sea_orm::ActiveValue::Set;

        let now = Utc::now();
        let txn = self.db.begin().await?;
        let user_log = entities::conversation_logs::ActiveModel {
            session_id: Set(session_id.to_string()),
            card_name: Set(card_name.to_string()),
            role: Set("user".to_string()),
            content: Set(user_message.to_string()),
            created_at: Set(now),
            ..Default::default()
        };
        let user_res = user_log.insert(&txn).await?;

        let assistant_log = entities::conversation_logs::ActiveModel {
            session_id: Set(session_id.to_string()),
            card_name: Set(card_name.to_string()),
            role: Set("assistant".to_string()),
            content: Set(assistant_response.to_string()),
            created_at: Set(now),
            ..Default::default()
        };
        let assistant_res = assistant_log.insert(&txn).await?;
        txn.commit().await?;

        Ok((user_res.id, assistant_res.id))
    }

    /// Spawns a fire-and-forget task that inserts a conversation log entry.
    ///
    /// Errors are logged at the `error` tracing level. Takes an `Arc<Self>`
    /// so the store outlives the spawned task.
    pub fn spawn_insert_log(
        store: &Arc<Self>,
        session_id: &str,
        card_name: &str,
        role: &str,
        content: &str,
    ) {
        let store = store.clone();
        let session_id = session_id.to_string();
        let card_name = card_name.to_string();
        let role = role.to_string();
        let content = content.to_string();
        tokio::spawn(async move {
            if let Err(e) = store
                .insert_log(&session_id, &card_name, &role, &content)
                .await
            {
                tracing::error!(component = "Memory", role = ?role, error = %e, "Failed to save log");
            }
        });
    }

    /// Returns all conversation logs for a session.
    pub async fn get_logs_by_session(
        &self,
        session_id: &str,
    ) -> Result<Vec<ConversationLogEntry>, EneMemoryError> {
        let rows = entities::conversation_logs::Entity::find()
            .filter(entities::conversation_logs::Column::SessionId.eq(session_id))
            .order_by_asc(entities::conversation_logs::Column::CreatedAt)
            .all(&self.db)
            .await?;

        Ok(rows
            .into_iter()
            .map(|row| ConversationLogEntry {
                role: row.role,
                content: row.content,
                created_at: row.created_at,
            })
            .collect())
    }

    // ── Sessions & Export/Import (#176) ─────────────────────────────────────

    /// Inserts a session metadata row, or refreshes `updated_at` if the
    /// `session_id` already exists.
    ///
    /// Only `updated_at` is bumped on conflict; `card_name`, `title`,
    /// `turn_count`, and `archived` are left untouched. Returns the row id.
    pub async fn upsert_session(
        &self,
        meta: &crate::session::NewSessionMeta,
    ) -> Result<i64, EneMemoryError> {
        use sea_orm::ActiveValue::Set;

        let now = Utc::now();
        let model = entities::session::ActiveModel {
            session_id: Set(meta.session_id.clone()),
            card_name: Set(meta.card_name.clone()),
            title: Set(meta.title.clone()),
            created_at: Set(now),
            updated_at: Set(now),
            archived: Set(0),
            turn_count: Set(0),
            ..Default::default()
        };

        entities::session::Entity::insert(model)
            .on_conflict(
                OnConflict::column(entities::session::Column::SessionId)
                    .update_column(entities::session::Column::UpdatedAt)
                    .to_owned(),
            )
            .exec(&self.db)
            .await?;

        let row = self
            .get_session(&meta.session_id)
            .await?
            .ok_or_else(|| EneMemoryError::Other("session row missing after upsert".to_string()))?;
        Ok(row.id)
    }

    /// Updates a session's `updated_at` timestamp and `turn_count`.
    ///
    /// No-op if the session does not exist.
    pub async fn touch_session(
        &self,
        session_id: &str,
        turn_count: i64,
    ) -> Result<(), EneMemoryError> {
        entities::session::Entity::update_many()
            .col_expr(
                entities::session::Column::UpdatedAt,
                Expr::value(Utc::now()),
            )
            .col_expr(
                entities::session::Column::TurnCount,
                Expr::value(turn_count),
            )
            .filter(entities::session::Column::SessionId.eq(session_id))
            .exec(&self.db)
            .await?;

        Ok(())
    }

    /// Returns the metadata for a single session, if present.
    pub async fn get_session(
        &self,
        session_id: &str,
    ) -> Result<Option<crate::session::SessionMeta>, EneMemoryError> {
        let row = entities::session::Entity::find()
            .filter(entities::session::Column::SessionId.eq(session_id))
            .one(&self.db)
            .await?;

        Ok(row.map(Into::into))
    }

    /// Lists sessions, newest `updated_at` first.
    ///
    /// Archived sessions are excluded unless `include_archived` is set.
    pub async fn list_sessions(
        &self,
        include_archived: bool,
        limit: usize,
    ) -> Result<Vec<crate::session::SessionMeta>, EneMemoryError> {
        let mut query = entities::session::Entity::find();
        if !include_archived {
            query = query.filter(entities::session::Column::Archived.eq(0));
        }
        let rows = query
            .order_by_desc(entities::session::Column::UpdatedAt)
            .limit(limit as u64)
            .all(&self.db)
            .await?;

        Ok(rows.into_iter().map(Into::into).collect())
    }

    /// Sets the archive flag for a session.
    ///
    /// Returns whether a row was actually updated (i.e. the session exists).
    pub async fn set_session_archived(
        &self,
        session_id: &str,
        archived: bool,
    ) -> Result<bool, EneMemoryError> {
        let res = entities::session::Entity::update_many()
            .col_expr(
                entities::session::Column::Archived,
                Expr::value(i32::from(archived)),
            )
            .filter(entities::session::Column::SessionId.eq(session_id))
            .exec(&self.db)
            .await?;

        Ok(res.rows_affected > 0)
    }

    /// Returns all conversation messages for a session, oldest first.
    pub async fn list_messages(
        &self,
        session_id: &str,
    ) -> Result<Vec<crate::export::ExportedMessage>, EneMemoryError> {
        let rows = entities::conversation_logs::Entity::find()
            .filter(entities::conversation_logs::Column::SessionId.eq(session_id))
            .order_by_asc(entities::conversation_logs::Column::CreatedAt)
            .all(&self.db)
            .await?;

        Ok(rows.into_iter().map(log_row_to_message).collect())
    }

    /// Case-insensitive substring search over conversation message content.
    ///
    /// Returns `(session_id, message)` pairs, paginated by `limit`/`offset`.
    /// The query is bound as a parameter (never concatenated into SQL), so it
    /// is injection-safe. An empty query returns an empty result set.
    pub async fn search_messages(
        &self,
        query: &str,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<(String, crate::export::ExportedMessage)>, EneMemoryError> {
        if query.is_empty() {
            return Ok(Vec::new());
        }

        use sea_orm::ExprTrait;

        let pattern = format!("%{}%", query.to_ascii_lowercase());
        let rows = entities::conversation_logs::Entity::find()
            .filter(
                sea_orm::sea_query::Func::lower(sea_orm::sea_query::Expr::col(
                    entities::conversation_logs::Column::Content,
                ))
                .like(pattern),
            )
            .order_by_desc(entities::conversation_logs::Column::CreatedAt)
            .limit(limit as u64)
            .offset(offset as u64)
            .all(&self.db)
            .await?;

        Ok(rows
            .into_iter()
            .map(|row| {
                let session_id = row.session_id.clone();
                (session_id, log_row_to_message(row))
            })
            .collect())
    }

    /// Assembles a redacted, self-contained export for one session.
    ///
    /// Message content is passed through [`crate::export::redact_secrets`] so
    /// obvious credentials never leave the store in the clear.
    ///
    /// # Limitation
    ///
    /// `tool_logs` is always empty: the `audit_log` table records a `turn_id`
    /// but has no column linking a row back to its session, so audit entries
    /// cannot be associated with a session reliably. Rather than guess (and
    /// risk leaking another session's tool calls or fabricating data), the
    /// export omits tool logs. A future schema revision could add a
    /// `session_id` column to `audit_log` to enable this.
    ///
    /// # Errors
    ///
    /// Returns [`EneMemoryError::Other`] if the session does not exist.
    pub async fn build_export(
        &self,
        session_id: &str,
    ) -> Result<crate::export::SessionExport, EneMemoryError> {
        let session = self
            .get_session(session_id)
            .await?
            .ok_or_else(|| EneMemoryError::Other(format!("session not found: {session_id}")))?;

        let messages = self
            .list_messages(session_id)
            .await?
            .into_iter()
            .map(|mut message| {
                message.content = crate::export::redact_secrets(&message.content);
                message
            })
            .collect();

        Ok(crate::export::SessionExport {
            format_version: crate::export::SESSION_EXPORT_FORMAT_VERSION,
            exported_at: Utc::now(),
            session,
            messages,
            tool_logs: Vec::new(),
        })
    }

    /// Imports a previously exported session, returning the new session row id.
    ///
    /// Conflict handling: if the incoming `session_id` already exists in the
    /// store, the session (and its messages) is imported under a freshly
    /// generated `session_id` (`imported-<uuid>`) so existing sessions are
    /// never overwritten silently. Otherwise the original `session_id` is
    /// preserved.
    ///
    /// # Errors
    ///
    /// Propagates any underlying store error from the inserts.
    pub async fn import_export(
        &self,
        export: &crate::export::SessionExport,
    ) -> Result<i64, EneMemoryError> {
        use sea_orm::ActiveModelTrait;
        use sea_orm::ActiveValue::Set;

        let target_session_id = if self
            .get_session(&export.session.session_id)
            .await?
            .is_some()
        {
            format!("imported-{}", uuid::Uuid::new_v4())
        } else {
            export.session.session_id.clone()
        };

        let new_meta = crate::session::NewSessionMeta {
            session_id: target_session_id.clone(),
            card_name: export.session.card_name.clone(),
            title: export.session.title.clone(),
        };
        let session_row_id = self.upsert_session(&new_meta).await?;

        let txn = self.db.begin().await?;
        for message in &export.messages {
            let model = entities::conversation_logs::ActiveModel {
                session_id: Set(target_session_id.clone()),
                card_name: Set(new_meta.card_name.clone()),
                role: Set(message.role.clone()),
                content: Set(message.content.clone()),
                created_at: Set(message.created_at),
                ..Default::default()
            };
            model.insert(&txn).await?;
        }
        entities::session::Entity::update_many()
            .col_expr(
                entities::session::Column::TurnCount,
                Expr::value(export.messages.len() as i64),
            )
            .filter(entities::session::Column::SessionId.eq(&target_session_id))
            .exec(&txn)
            .await?;
        txn.commit().await?;

        Ok(session_row_id)
    }
}
