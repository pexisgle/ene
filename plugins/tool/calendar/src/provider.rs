use crate::action;
use crate::state::CalendarState;
use async_trait::async_trait;
use ene_plugin::{ActionSetProvider, ToolAction};
use ene_plugin_proto::{SandboxConfigData, ToolError, ToolProvider, ToolSpec};
use std::sync::Arc;

/// Built on [`ActionSetProvider`]: `list_specs`/`call_tool` dispatch is
/// handled generically, and the calendar-specific state — DB sandbox
/// socket/token and the approval gate — is threaded in via hooks.
pub struct CalendarToolProvider {
    inner: ActionSetProvider,
}

impl CalendarToolProvider {
    #[must_use]
    pub fn new() -> Self {
        let state = Arc::new(CalendarState::new());
        let actions: Vec<Box<dyn ToolAction>> = vec![
            Box::new(action::ListCalendarsAction::new(state.clone())),
            Box::new(action::AddCalendarAction::new(state.clone())),
            Box::new(action::SetPermissionAction::new(state.clone())),
            Box::new(action::RemoveAccountAction::new(state.clone())),
            Box::new(action::ListEventsAction::new(state.clone())),
            Box::new(action::CreateEventAction::new(state.clone())),
            Box::new(action::UpdateEventAction::new(state.clone())),
            Box::new(action::CancelEventAction::new(state.clone())),
            Box::new(action::FindFreeSlotsAction::new(state.clone())),
        ];

        let session_state = state.clone();
        let sandbox_state = state.clone();
        let approve_state = state.clone();
        let allow_state = state.clone();
        let revoke_state = state;
        let inner = ActionSetProvider::new(actions)
            .with_set_call_context_hook(move |conv_id, turn_id| {
                session_state.gate().on_call_context(conv_id, turn_id);
            })
            .with_sandbox_hook(move |sandbox: &SandboxConfigData| {
                if let Some(socket) = &sandbox.db_socket {
                    sandbox_state.set_db_socket(socket.clone());
                }
                sandbox_state.set_db_auth_token(sandbox.db_auth_token.clone());
            })
            .with_approve_permission_hook(move |request_id| {
                approve_state.gate().approve_request(request_id);
            })
            .with_allow_pattern_hook(move |action, target_pattern| {
                allow_state.gate().allow_pattern(action, target_pattern);
            })
            .with_revoke_pattern_hook(move |action, target_pattern| {
                revoke_state.gate().revoke_pattern(action, target_pattern);
            });

        Self { inner }
    }
}

impl Default for CalendarToolProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ToolProvider for CalendarToolProvider {
    fn list_specs(&self) -> Vec<ToolSpec> {
        self.inner.list_specs()
    }

    async fn call_tool(&self, name: &str, arguments: &str) -> Result<String, ToolError> {
        self.inner.call_tool(name, arguments).await
    }

    fn set_call_context(&self, ctx: &ene_plugin_proto::CallContext) {
        self.inner.set_call_context(ctx);
    }

    fn set_sandbox(&self, sandbox: &SandboxConfigData) {
        self.inner.set_sandbox(sandbox);
    }

    fn approve_permission(&self, request_id: &str) {
        self.inner.approve_permission(request_id);
    }

    fn allow_pattern(&self, action: &str, target_pattern: &str) {
        self.inner.allow_pattern(action, target_pattern);
    }

    fn revoke_pattern(&self, action: &str, target_pattern: &str) {
        self.inner.revoke_pattern(action, target_pattern);
    }
}
