mod add_calendar;
mod cancel_event;
mod create_event;
mod find_free_slots;
mod list_calendars;
mod list_events;
mod remove_account;
mod set_permission;
mod update_event;

pub use add_calendar::AddCalendarAction;
pub use cancel_event::CancelEventAction;
pub use create_event::CreateEventAction;
pub use find_free_slots::FindFreeSlotsAction;
pub use list_calendars::ListCalendarsAction;
pub use list_events::ListEventsAction;
pub use remove_account::RemoveAccountAction;
pub use set_permission::SetPermissionAction;
pub use update_event::UpdateEventAction;

#[cfg(test)]
mod tests;

use crate::store::{CalendarStoreError, parse_rfc3339_ms};
use ene_plugin::prelude::*;

fn ok_json<T: serde::Serialize>(value: &T) -> Result<String, ToolError> {
    serde_json::to_string_pretty(value)
        .map_err(|e| ToolError::internal(format!("json serialization failed: {e}")))
}

/// Maps a [`CalendarStoreError`] to the appropriate [`ToolError`] variant.
///
/// Argument-level problems (unknown ids, bad timestamps, duplicates, invalid
/// status) map to `InvalidArguments`; permission denials map to
/// `permission_denied` (fail-closed, surfaced verbatim to the LLM so it can
/// adapt); database/transport problems map to `Internal`.
fn store_err(e: &CalendarStoreError) -> ToolError {
    match e {
        CalendarStoreError::AccountNotFound(_)
        | CalendarStoreError::EventNotFound(_)
        | CalendarStoreError::EventNotFoundInAccount { .. }
        | CalendarStoreError::DuplicateName(_)
        | CalendarStoreError::InvalidTimeRange
        | CalendarStoreError::InvalidTimestamp(_)
        | CalendarStoreError::InvalidStatus(_)
        | CalendarStoreError::InvalidNewStatus(_)
        | CalendarStoreError::EmptyAccountName => ToolError::InvalidArguments {
            message: e.to_string(),
        },
        CalendarStoreError::ReadDenied(_) | CalendarStoreError::WriteDenied(_) => {
            ToolError::permission_denied(e.to_string())
        }
        CalendarStoreError::Db(_)
        | CalendarStoreError::MissingAuthToken
        | CalendarStoreError::UnknownProvider(_)
        | CalendarStoreError::CorruptRow(_) => ToolError::internal(e.to_string()),
    }
}

fn parse_bound(value: Option<&str>) -> Result<Option<i64>, ToolError> {
    match value {
        Some(v) if v.trim().is_empty() => Ok(None),
        Some(v) => parse_rfc3339_ms(v)
            .map(Some)
            .map_err(|e| ToolError::InvalidArguments {
                message: e.to_string(),
            }),
        None => Ok(None),
    }
}
