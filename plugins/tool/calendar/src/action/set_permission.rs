use super::ok_json;
use crate::approval::actions::CALENDAR_PERMISSION;
use crate::state::CalendarState;
use ene_plugin::prelude::*;
use std::sync::Arc;

fn default_state() -> Arc<CalendarState> {
    Arc::new(CalendarState::new())
}

#[derive(Clone, Deserialize, JsonSchema, ToolAction)]
#[tool(
    namespace = "calendar",
    name = "set_permission",
    summary = "Change the read/write permissions of a calendar account.",
    description = "Sets the read_allowed and/or write_allowed flags of an existing calendar. Reads are denied on calendars without read permission; writes are always denied on calendars without write permission, in addition to per-operation approval. Requires user approval before the change is applied.",
    category = "Utility",
    keywords_primary = "calendar, permission, access, allow, deny, grant, revoke",
    side_effects = "Idempotent"
)]
/// Action to change a calendar's read/write permission flags.
pub struct SetPermissionAction {
    /// Id of the calendar returned by ``calendar.list_calendars``.
    calendar_id: String,
    /// Whether read operations should be allowed.
    read_allowed: Option<bool>,
    /// Whether write operations should be allowed.
    write_allowed: Option<bool>,

    #[tool(skip)]
    #[serde(skip, default = "default_state")]
    state: Arc<CalendarState>,
}

impl SetPermissionAction {
    /// Creates a new `SetPermissionAction`.
    #[must_use]
    pub const fn new(state: Arc<CalendarState>) -> Self {
        Self {
            calendar_id: String::new(),
            read_allowed: None,
            write_allowed: None,
            state,
        }
    }

    async fn run(&self) -> Result<String, ToolError> {
        if self.read_allowed.is_none() && self.write_allowed.is_none() {
            return Err(ToolError::InvalidArguments {
                message: "set at least one of read_allowed / write_allowed".to_string(),
            });
        }
        let store = self.state.ensure_store().await?;
        let account = store
            .get_account(&self.calendar_id)
            .await
            .map_err(|e| super::store_err(&e))?;

        let read_label = self
            .read_allowed
            .map_or_else(|| "unchanged".to_string(), |v| v.to_string());
        let write_label = self
            .write_allowed
            .map_or_else(|| "unchanged".to_string(), |v| v.to_string());
        let description = format!(
            "Change permissions on calendar '{}': read_allowed={read_label}, write_allowed={write_label}",
            account.name,
        );
        let target = format!("calendar:{}", account.id);
        self.state
            .gate()
            .check(CALENDAR_PERMISSION, &target, &description)?;

        let updated = store
            .set_permissions(&self.calendar_id, self.read_allowed, self.write_allowed)
            .await
            .map_err(|e| super::store_err(&e))?;
        ok_json(&updated)
    }
}
