use super::ok_json;
use crate::state::CalendarState;
use crate::store::parse_rfc3339_ms;
use ene_plugin::prelude::*;
use std::sync::Arc;

fn default_state() -> Arc<CalendarState> {
    Arc::new(CalendarState::new())
}

#[derive(Clone, Deserialize, JsonSchema, ToolAction)]
#[tool(
    namespace = "calendar",
    name = "find_free_slots",
    summary = "Find free time slots of a given length in a calendar.",
    description = "Searches a calendar for contiguous free intervals of at least duration_min minutes within the given window (RFC3339 with offset). Returned slots are aligned to the window start in steps of duration_min; each slot is free of events for its whole length. Requires read permission on the calendar.",
    category = "Utility",
    keywords_primary = "calendar, free, available, slot, schedule, meeting, find",
    side_effects = "ReadOnly"
)]
pub struct FindFreeSlotsAction {
    /// Id of the calendar returned by ``calendar.list_calendars``.
    calendar_id: String,
    /// Window start (RFC3339 with offset, e.g. 2026-08-03T09:00:00+09:00).
    start: String,
    /// Window end (RFC3339 with offset); must be after start.
    end: String,
    /// Minimum slot length in minutes (must be at least 1).
    duration_min: u64,

    #[tool(skip)]
    #[serde(skip, default = "default_state")]
    state: Arc<CalendarState>,
}

impl FindFreeSlotsAction {
    #[must_use]
    pub const fn new(state: Arc<CalendarState>) -> Self {
        Self {
            calendar_id: String::new(),
            start: String::new(),
            end: String::new(),
            duration_min: 0,
            state,
        }
    }

    async fn run(&self) -> Result<String, ToolError> {
        if self.duration_min == 0 {
            return Err(ToolError::InvalidArguments {
                message: "duration_min must be at least 1".to_string(),
            });
        }
        let start_ms = parse_rfc3339_ms(&self.start).map_err(|e| ToolError::InvalidArguments {
            message: e.to_string(),
        })?;
        let end_ms = parse_rfc3339_ms(&self.end).map_err(|e| ToolError::InvalidArguments {
            message: e.to_string(),
        })?;
        if start_ms >= end_ms {
            return Err(ToolError::InvalidArguments {
                message: "end must be after start".to_string(),
            });
        }

        let store = self.state.ensure_store().await?;
        let account = store
            .require_readable(&self.calendar_id)
            .await
            .map_err(|e| super::store_err(&e))?;

        let provider = self
            .state
            .registry()
            .resolve(account.kind)
            .map_err(|e| super::store_err(&e))?;
        let slots = provider
            .find_free_slots(
                &store,
                &self.calendar_id,
                start_ms,
                end_ms,
                self.duration_min,
            )
            .await
            .map_err(|e| super::store_err(&e))?;

        let body = serde_json::json!({
            "summary": {
                "calendar": account.name,
                "count": slots.len(),
                "duration_min": self.duration_min,
            },
            "slots": slots,
        });
        ok_json(&body)
    }
}
