//! Audit-log queries (#177).

use super::{EneMemoryError, MemoryStore};
use crate::entities;
use chrono::Utc;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QueryOrder, QuerySelect};
use std::sync::Arc;

fn audit_row_to_entry(row: entities::audit_log::Model) -> crate::audit::AuditEntry {
    crate::audit::AuditEntry {
        id: row.id,
        turn_id: row.turn_id,
        tool_name: row.tool_name,
        action: row.action,
        target: row.target,
        decision: crate::audit::AuditDecision::parse(&row.decision),
        success: row.success != 0,
        redacted_args: row.redacted_args,
        created_at: row.created_at,
    }
}

impl MemoryStore {
    // ── Audit Log (#177) ──────────────────────────────────────────────────────

    /// Records a single audited tool call, redacting sensitive arguments.
    pub async fn insert_audit_entry(
        &self,
        entry: &crate::audit::NewAuditEntry,
    ) -> Result<i64, EneMemoryError> {
        use sea_orm::ActiveModelTrait;
        use sea_orm::ActiveValue::Set;

        let now = Utc::now();
        let model = entities::audit_log::ActiveModel {
            turn_id: Set(entry.turn_id.clone()),
            tool_name: Set(entry.tool_name.clone()),
            action: Set(entry.action.clone()),
            target: Set(entry.target.clone()),
            decision: Set(entry.decision.as_str().to_string()),
            success: Set(i32::from(entry.success)),
            redacted_args: Set(crate::audit::redact_arguments(&entry.arguments)),
            created_at: Set(now),
            ..Default::default()
        };

        let res = model.insert(&self.db).await?;
        Ok(res.id)
    }

    /// Spawns a fire-and-forget task that records an audited tool call.
    ///
    /// Errors are logged at the `error` tracing level. Takes an `Arc<Self>`
    /// so the store outlives the spawned task.
    pub fn spawn_insert_audit_entry(store: &Arc<Self>, entry: crate::audit::NewAuditEntry) {
        let store = store.clone();
        tokio::spawn(async move {
            if let Err(e) = store.insert_audit_entry(&entry).await {
                tracing::error!(
                    component = "AuditLog",
                    tool = %entry.tool_name,
                    error = %e,
                    "Failed to record audit entry"
                );
            }
        });
    }

    /// Returns the most recent audit entries (newest first).
    pub async fn list_audit_entries(
        &self,
        limit: usize,
    ) -> Result<Vec<crate::audit::AuditEntry>, EneMemoryError> {
        let rows = entities::audit_log::Entity::find()
            .order_by_desc(entities::audit_log::Column::CreatedAt)
            .limit(limit as u64)
            .all(&self.db)
            .await?;

        Ok(rows.into_iter().map(audit_row_to_entry).collect())
    }

    /// Returns audit entries filtered by tool name (newest first).
    pub async fn list_audit_entries_by_tool(
        &self,
        tool_name: &str,
        limit: usize,
    ) -> Result<Vec<crate::audit::AuditEntry>, EneMemoryError> {
        let rows = entities::audit_log::Entity::find()
            .filter(entities::audit_log::Column::ToolName.eq(tool_name))
            .order_by_desc(entities::audit_log::Column::CreatedAt)
            .limit(limit as u64)
            .all(&self.db)
            .await?;

        Ok(rows.into_iter().map(audit_row_to_entry).collect())
    }
}
