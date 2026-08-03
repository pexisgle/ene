use super::ok_json;
use crate::approval::actions::CALENDAR_ADD;
use crate::state::CalendarState;
use crate::store::CalendarKind;
use ene_plugin::prelude::*;
use std::sync::Arc;

fn default_state() -> Arc<CalendarState> {
    Arc::new(CalendarState::new())
}

#[derive(Clone, Deserialize, JsonSchema, ToolAction)]
#[tool(
    namespace = "calendar",
    name = "add_calendar",
    summary = "Add a new calendar account.",
    description = "Creates a new calendar account. New accounts allow reading by default but deny writes; enable writes with `calendar.set_permission`. Requires user approval before the calendar is created.",
    category = "Utility",
    keywords_primary = "calendar, add, create, new, account",
    side_effects = "Idempotent"
)]
/// Action to add a new calendar account.
pub struct AddCalendarAction {
    /// Display name of the calendar; must be unique.
    name: String,
    /// Account kind; only `local` is supported today.
    #[serde(default)]
    kind: String,

    #[tool(skip)]
    #[serde(skip, default = "default_state")]
    state: Arc<CalendarState>,
}

impl AddCalendarAction {
    /// Creates a new `AddCalendarAction`.
    #[must_use]
    pub const fn new(state: Arc<CalendarState>) -> Self {
        Self {
            name: String::new(),
            kind: String::new(),
            state,
        }
    }

    async fn run(&self) -> Result<String, ToolError> {
        let kind = match self.kind.trim() {
            "" | "local" => CalendarKind::Local,
            other => {
                return Err(ToolError::InvalidArguments {
                    message: format!(
                        "unsupported calendar kind '{other}'; only 'local' is available"
                    ),
                });
            }
        };
        // Deterministic id derived from the name: the approval-gate retry
        // re-invokes this action with identical arguments, so the gate target
        // (`calendar:<id>`) must be identical across the retry for the
        // recorded approval to match. It also makes `add_calendar` idempotent
        // per name, matching its `Idempotent` side-effect declaration.
        let account_id = uuid::Uuid::new_v5(
            &uuid::Uuid::NAMESPACE_URL,
            format!("calendar:{}", self.name.trim()).as_bytes(),
        )
        .to_string();
        let target = format!("calendar:{account_id}");
        let description = format!(
            "Add a new calendar named '{}' (kind: {})",
            self.name.trim(),
            kind,
        );
        self.state
            .gate()
            .check(CALENDAR_ADD, &target, &description)?;

        let store = self.state.ensure_store().await?;
        let account = store
            .add_account(&account_id, &self.name, kind)
            .await
            .map_err(|e| super::store_err(&e))?;
        ok_json(&account)
    }
}
