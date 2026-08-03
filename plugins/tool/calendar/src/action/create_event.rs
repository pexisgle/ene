use super::ok_json;
use crate::approval::actions::CALENDAR_WRITE;
use crate::state::CalendarState;
use crate::store::{CalendarEventInput, parse_rfc3339_ms};
use ene_plugin::prelude::*;
use std::sync::Arc;

fn default_state() -> Arc<CalendarState> {
    Arc::new(CalendarState::new())
}

#[derive(Clone, Deserialize, JsonSchema, ToolAction)]
#[tool(
    namespace = "calendar",
    name = "create_event",
    summary = "Create an event in a calendar.",
    description = "Adds an event to a calendar. The event time window is given as RFC3339 timestamps with offset (start and end); an optional IANA timezone name is used for display. Requires write permission on the calendar and explicit user approval showing the timezone, the target calendar, and the event content before the event is created.",
    category = "Utility",
    keywords_primary = "calendar, create, add, event, appointment, schedule",
    side_effects = "Network { external: true }"
)]
/// Action to create an event in a calendar.
pub struct CreateEventAction {
    /// Id of the calendar returned by ``calendar.list_calendars``.
    calendar_id: String,
    /// Human-readable event title.
    title: String,
    /// Free-form notes.
    #[serde(default)]
    description: String,
    /// Location string.
    #[serde(default)]
    location: String,
    /// Start time (RFC3339 with offset, e.g. 2026-08-03T10:00:00+09:00).
    start: String,
    /// End time (RFC3339 with offset); must be after start.
    end: String,
    /// IANA timezone name for display; defaults to the start offset.
    #[serde(default)]
    timezone: String,
    /// Attendee identifiers.
    #[serde(default)]
    attendees: Vec<String>,
    /// Event status: 'confirmed' (default) or 'tentative'.
    #[serde(default)]
    status: String,

    #[tool(skip)]
    #[serde(skip, default = "default_state")]
    state: Arc<CalendarState>,
}

impl CreateEventAction {
    /// Creates a new `CreateEventAction`.
    #[must_use]
    pub const fn new(state: Arc<CalendarState>) -> Self {
        Self {
            calendar_id: String::new(),
            title: String::new(),
            description: String::new(),
            location: String::new(),
            start: String::new(),
            end: String::new(),
            timezone: String::new(),
            attendees: Vec::new(),
            status: String::new(),
            state,
        }
    }

    async fn run(&self) -> Result<String, ToolError> {
        if self.title.trim().is_empty() {
            return Err(ToolError::InvalidArguments {
                message: "title must not be empty".to_string(),
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
            .require_writable(&self.calendar_id)
            .await
            .map_err(|e| super::store_err(&e))?;

        let input = CalendarEventInput {
            title: self.title.clone(),
            description: self.description.clone(),
            location: self.location.clone(),
            start: self.start.clone(),
            end: self.end.clone(),
            timezone: self.timezone.clone(),
            attendees: self.attendees.clone(),
            status: self.status.clone(),
        };
        let timezone = if self.timezone.trim().is_empty() {
            chrono::DateTime::parse_from_rfc3339(&self.start)
                .map_or_else(|_| "UTC".to_string(), |dt| dt.offset().to_string())
        } else {
            self.timezone.clone()
        };
        let preview = format!(
            "Create event '{}' on calendar '{}' (timezone {}): {} - {}",
            self.title.trim(),
            account.name,
            timezone,
            self.start,
            self.end,
        );
        let target = format!("calendar:{}", account.id);
        self.state.gate().check(CALENDAR_WRITE, &target, &preview)?;

        let provider = self
            .state
            .registry()
            .resolve(account.kind)
            .map_err(|e| super::store_err(&e))?;
        let event = provider
            .create_event(&store, &self.calendar_id, &input)
            .await
            .map_err(|e| super::store_err(&e))?;
        ok_json(&event)
    }
}
