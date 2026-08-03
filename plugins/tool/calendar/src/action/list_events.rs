use super::{ok_json, parse_bound};
use crate::state::CalendarState;
use ene_plugin::prelude::*;
use std::sync::Arc;

fn default_state() -> Arc<CalendarState> {
    Arc::new(CalendarState::new())
}

#[derive(Clone, Deserialize, JsonSchema, ToolAction)]
#[tool(
    namespace = "calendar",
    name = "list_events",
    summary = "List events of a calendar within an optional time window.",
    description = "Returns the events of one calendar between the optional start and end bounds (RFC3339 with offset). Without bounds, all events from the calendar are returned. Cancelled events are excluded unless include_cancelled is true. Requires read permission on the calendar.",
    category = "Utility",
    keywords_primary = "calendar, event, list, schedule, agenda, appointments",
    side_effects = "ReadOnly"
)]
/// Action to list events of a calendar.
pub struct ListEventsAction {
    /// Id of the calendar returned by ``calendar.list_calendars``.
    calendar_id: String,
    /// Optional window start (RFC3339, e.g. 2026-08-03T00:00:00+09:00).
    #[serde(default)]
    start: Option<String>,
    /// Optional window end (RFC3339); events starting before this bound are included.
    #[serde(default)]
    end: Option<String>,
    /// Include events with status 'cancelled' (default false).
    #[serde(default)]
    include_cancelled: bool,

    #[tool(skip)]
    #[serde(skip, default = "default_state")]
    state: Arc<CalendarState>,
}

impl ListEventsAction {
    /// Creates a new `ListEventsAction`.
    #[must_use]
    pub const fn new(state: Arc<CalendarState>) -> Self {
        Self {
            calendar_id: String::new(),
            start: None,
            end: None,
            include_cancelled: false,
            state,
        }
    }

    async fn run(&self) -> Result<String, ToolError> {
        let store = self.state.ensure_store().await?;
        let account = store
            .require_readable(&self.calendar_id)
            .await
            .map_err(|e| super::store_err(&e))?;

        let start_ms = parse_bound(self.start.as_deref())?;
        let end_ms = parse_bound(self.end.as_deref())?;
        if let (Some(start), Some(end)) = (start_ms, end_ms)
            && start >= end
        {
            return Err(ToolError::InvalidArguments {
                message: "start must be before end".to_string(),
            });
        }

        let provider = self
            .state
            .registry()
            .resolve(account.kind)
            .map_err(|e| super::store_err(&e))?;
        let events = provider
            .list_events(
                &store,
                &self.calendar_id,
                start_ms,
                end_ms,
                self.include_cancelled,
            )
            .await
            .map_err(|e| super::store_err(&e))?;

        let body = serde_json::json!({
            "summary": {
                "calendar": account.name,
                "count": events.len(),
            },
            "events": events,
        });
        ok_json(&body)
    }
}
