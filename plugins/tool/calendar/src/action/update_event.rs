use super::ok_json;
use crate::approval::actions::CALENDAR_WRITE;
use crate::state::CalendarState;
use crate::store::{CalendarEventChanges, format_event_changes, parse_rfc3339_ms};
use ene_plugin::prelude::*;
use std::sync::Arc;

fn default_state() -> Arc<CalendarState> {
    Arc::new(CalendarState::new())
}

#[derive(Clone, Deserialize, JsonSchema, ToolAction)]
#[tool(
    namespace = "calendar",
    name = "update_event",
    summary = "Update an existing calendar event.",
    description = "Applies partial changes to an existing event; only the provided fields are modified. Timestamps are RFC3339 with offset. Requires write permission on the calendar and explicit user approval showing a diff of the change (before -> after) before the event is modified.",
    category = "Utility",
    keywords_primary = "calendar, update, edit, change, modify, reschedule, event",
    side_effects = "Network { external: true }"
)]
/// Action to update an existing calendar event.
pub struct UpdateEventAction {
    /// Id of the calendar returned by ``calendar.list_calendars``.
    calendar_id: String,
    /// Id of the event returned by ``calendar.list_events``.
    event_id: String,
    /// New title.
    title: Option<String>,
    /// New notes.
    description: Option<String>,
    /// New location.
    location: Option<String>,
    /// New start time (RFC3339 with offset).
    start: Option<String>,
    /// New end time (RFC3339 with offset).
    end: Option<String>,
    /// New timezone label.
    timezone: Option<String>,
    /// New attendee list.
    attendees: Option<Vec<String>>,
    /// New status: 'confirmed', 'tentative', or 'cancelled'.
    status: Option<String>,

    #[tool(skip)]
    #[serde(skip, default = "default_state")]
    state: Arc<CalendarState>,
}

impl UpdateEventAction {
    /// Creates a new `UpdateEventAction`.
    #[must_use]
    pub const fn new(state: Arc<CalendarState>) -> Self {
        Self {
            calendar_id: String::new(),
            event_id: String::new(),
            title: None,
            description: None,
            location: None,
            start: None,
            end: None,
            timezone: None,
            attendees: None,
            status: None,
            state,
        }
    }

    async fn run(&self) -> Result<String, ToolError> {
        let changes = CalendarEventChanges {
            title: self.title.clone(),
            description: self.description.clone(),
            location: self.location.clone(),
            start: self.start.clone(),
            end: self.end.clone(),
            timezone: self.timezone.clone(),
            attendees: self.attendees.clone(),
            status: self.status.clone(),
        };
        let has_change = changes.title.is_some()
            || changes.description.is_some()
            || changes.location.is_some()
            || changes.start.is_some()
            || changes.end.is_some()
            || changes.timezone.is_some()
            || changes.attendees.is_some()
            || changes.status.is_some();
        if !has_change {
            return Err(ToolError::InvalidArguments {
                message: "nothing to update: provide at least one field".to_string(),
            });
        }
        for ts in [&changes.start, &changes.end].into_iter().flatten() {
            parse_rfc3339_ms(ts).map_err(|e| ToolError::InvalidArguments {
                message: e.to_string(),
            })?;
        }

        let store = self.state.ensure_store().await?;
        let account = store
            .require_writable(&self.calendar_id)
            .await
            .map_err(|e| super::store_err(&e))?;
        let current = store
            .get_event(&self.calendar_id, &self.event_id)
            .await
            .map_err(|e| super::store_err(&e))?;

        let preview = format_event_changes(&account.name, &current, &changes);
        let target = format!("calendar:{}#{}", account.id, current.id);
        self.state.gate().check(CALENDAR_WRITE, &target, &preview)?;

        let provider = self
            .state
            .registry()
            .resolve(account.kind)
            .map_err(|e| super::store_err(&e))?;
        let updated = provider
            .update_event(&store, &self.calendar_id, &self.event_id, &changes)
            .await
            .map_err(|e| super::store_err(&e))?;
        ok_json(&updated)
    }
}
