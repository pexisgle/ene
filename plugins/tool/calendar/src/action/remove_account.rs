use super::ok_json;
use crate::approval::actions::CALENDAR_DELETE;
use crate::state::CalendarState;
use ene_plugin::prelude::*;
use std::sync::Arc;

fn default_state() -> Arc<CalendarState> {
    Arc::new(CalendarState::new())
}

#[derive(Clone, Deserialize, JsonSchema, ToolAction)]
#[tool(
    namespace = "calendar",
    name = "remove_account",
    summary = "Unlink a calendar account and delete all of its events.",
    description = "Permanently removes a calendar account and every event it holds. The removal is applied immediately and cannot be undone. Requires user approval before anything is deleted.",
    category = "Utility",
    keywords_primary = "calendar, remove, unlink, disconnect, delete, account",
    side_effects = "Destructive"
)]
/// Action to unlink a calendar account.
pub struct RemoveAccountAction {
    /// Id of the calendar returned by ``calendar.list_calendars``.
    calendar_id: String,

    #[tool(skip)]
    #[serde(skip, default = "default_state")]
    state: Arc<CalendarState>,
}

impl RemoveAccountAction {
    /// Creates a new `RemoveAccountAction`.
    #[must_use]
    pub const fn new(state: Arc<CalendarState>) -> Self {
        Self {
            calendar_id: String::new(),
            state,
        }
    }

    async fn run(&self) -> Result<String, ToolError> {
        let store = self.state.ensure_store().await?;
        let account = store
            .get_account(&self.calendar_id)
            .await
            .map_err(|e| super::store_err(&e))?;

        let target = format!("calendar:{}", account.id);
        let description = format!(
            "Unlink calendar '{}' (kind: {}) and delete all of its events",
            account.name, account.kind,
        );
        self.state
            .gate()
            .check(CALENDAR_DELETE, &target, &description)?;

        store
            .remove_account(&self.calendar_id)
            .await
            .map_err(|e| super::store_err(&e))?;
        ok_json(&serde_json::json!({
            "removed": account.name,
            "calendar_id": account.id,
        }))
    }
}
