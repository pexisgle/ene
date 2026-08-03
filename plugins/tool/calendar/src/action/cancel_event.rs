use super::ok_json;
use crate::approval::actions::CALENDAR_DELETE;
use crate::state::CalendarState;
use crate::store::format_event_window;
use ene_plugin::prelude::*;
use std::sync::Arc;

fn default_state() -> Arc<CalendarState> {
    Arc::new(CalendarState::new())
}

#[derive(Clone, Deserialize, JsonSchema, ToolAction)]
#[tool(
    namespace = "calendar",
    name = "cancel_event",
    summary = "Cancel an event and remove it from the calendar.",
    description = "Cancels an event by removing it from the calendar. Requires write permission on the calendar and explicit user approval showing the event before it is removed.",
    category = "Utility",
    keywords_primary = "calendar, cancel, delete, remove, event, appointment",
    side_effects = "Destructive"
)]
/// Action to cancel (remove) an event.
pub struct CancelEventAction {
    /// Id of the calendar returned by ``calendar.list_calendars``.
    calendar_id: String,
    /// Id of the event returned by ``calendar.list_events``.
    event_id: String,

    #[tool(skip)]
    #[serde(skip, default = "default_state")]
    state: Arc<CalendarState>,
}

impl CancelEventAction {
    /// Creates a new `CancelEventAction`.
    #[must_use]
    pub const fn new(state: Arc<CalendarState>) -> Self {
        Self {
            calendar_id: String::new(),
            event_id: String::new(),
            state,
        }
    }

    async fn run(&self) -> Result<String, ToolError> {
        let store = self.state.ensure_store().await?;
        let account = store
            .require_writable(&self.calendar_id)
            .await
            .map_err(|e| super::store_err(&e))?;
        let event = store
            .get_event(&self.calendar_id, &self.event_id)
            .await
            .map_err(|e| super::store_err(&e))?;

        let target = format!("calendar:{}#{}", account.id, event.id);
        let description = format!(
            "Cancel event '{}' on calendar '{}': {}",
            event.title,
            account.name,
            format_event_window(&event),
        );
        self.state
            .gate()
            .check(CALENDAR_DELETE, &target, &description)?;

        let provider = self
            .state
            .registry()
            .resolve(account.kind)
            .map_err(|e| super::store_err(&e))?;
        let removed = provider
            .cancel_event(&store, &self.calendar_id, &self.event_id)
            .await
            .map_err(|e| super::store_err(&e))?;
        ok_json(&serde_json::json!({
            "cancelled": removed.title,
            "event_id": removed.id,
            "calendar_id": removed.account_id,
        }))
    }
}
