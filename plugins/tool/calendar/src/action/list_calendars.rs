use super::ok_json;
use crate::state::CalendarState;
use ene_plugin::prelude::*;
use std::sync::Arc;

fn default_state() -> Arc<CalendarState> {
    Arc::new(CalendarState::new())
}

#[derive(Clone, Deserialize, JsonSchema, ToolAction)]
#[tool(
    namespace = "calendar",
    name = "list_calendars",
    summary = "List the user's calendar accounts and their permission flags.",
    description = "Lists every calendar account with its id, name, kind, and current read/write permission flags. Use the returned ids in other calendar.* actions. Permission changes require the separate `calendar.set_permission` action.",
    category = "Utility",
    keywords_primary = "calendar, calendars, account, list",
    side_effects = "ReadOnly"
)]
/// Action to list all calendar accounts.
pub struct ListCalendarsAction {
    #[tool(skip)]
    #[serde(skip, default = "default_state")]
    state: Arc<CalendarState>,
}

impl ListCalendarsAction {
    /// Creates a new `ListCalendarsAction`.
    #[must_use]
    pub const fn new(state: Arc<CalendarState>) -> Self {
        Self { state }
    }

    async fn run(&self) -> Result<String, ToolError> {
        let store = self.state.ensure_store().await?;
        let accounts = store
            .list_accounts()
            .await
            .map_err(|e| super::store_err(&e))?;
        let body = serde_json::json!({
            "summary": {
                "total": accounts.len(),
            },
            "calendars": accounts,
        });
        ok_json(&body)
    }
}
