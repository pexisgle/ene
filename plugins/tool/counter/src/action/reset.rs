use crate::approval::actions::COUNTER_RESET;
use crate::provider::CounterState;
use ene_plugin::prelude::*;
use std::sync::Arc;

/// Resets the session counter to zero.
#[derive(Debug, Clone, Deserialize, JsonSchema, ToolAction)]
#[tool(
    namespace = "counter",
    name = "reset",
    summary = "Reset all session counters to zero.",
    description = "Deletes every counter row from the shared database, resetting all sessions to zero. The previous values cannot be recovered, so the call requires explicit user approval. Once approved for the turn, the reset runs and every counter starts again at 0.",
    category = "Utility",
    keywords_primary = "counter, count, reset, clear, delete",
    side_effects = "Destructive"
)]
pub struct ResetAction {
    #[tool(skip)]
    #[serde(skip, default = "default_state")]
    state: Arc<CounterState>,
}

impl ResetAction {
    /// Creates a new `ResetAction`.
    #[must_use]
    pub const fn new(state: Arc<CounterState>) -> Self {
        Self { state }
    }

    async fn run(&self) -> Result<String, ToolError> {
        self.state.gate().check(
            COUNTER_RESET,
            "counter:reset",
            "Delete all counter rows and reset every session counter to zero",
        )?;
        let store = self.state.ensure_store().await?;
        store.reset().await?;
        Ok(serde_json::json!({ "reset": true }).to_string())
    }
}

fn default_state() -> Arc<CounterState> {
    Arc::new(CounterState::new())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::InMemoryCounterStore;

    async fn action_with_store() -> (ResetAction, Arc<CounterState>) {
        let state = Arc::new(CounterState::new());
        state
            .set_test_store(Arc::new(InMemoryCounterStore::default()))
            .await;
        state.set_session_id("session-1");
        (ResetAction::new(state.clone()), state)
    }

    #[tokio::test]
    async fn unapproved_reset_requires_permission() {
        let (action, _state) = action_with_store().await;
        let err = action.run().await.unwrap_err();
        assert!(matches!(err, ToolError::PermissionRequired { .. }));
    }

    #[tokio::test]
    async fn approved_reset_succeeds() {
        let (action, state) = action_with_store().await;
        let err = action.run().await.unwrap_err();
        let ToolError::PermissionRequired { request_id, .. } = err else {
            unreachable!("unapproved reset must require permission");
        };
        state.gate().approve_request(&request_id);
        let out = action.run().await.unwrap();
        assert_eq!(out, r#"{"reset":true}"#);
    }

    #[tokio::test]
    async fn approval_expires_on_new_turn() {
        let (action, state) = action_with_store().await;
        let err = action.run().await.unwrap_err();
        let ToolError::PermissionRequired { request_id, .. } = err else {
            unreachable!("unapproved reset must require permission");
        };
        state.gate().approve_request(&request_id);
        action.run().await.unwrap();
        state.gate().on_call_context("conv-1", Some("turn-2"));
        let err = action.run().await.unwrap_err();
        assert!(
            matches!(err, ToolError::PermissionRequired { .. }),
            "approval must not outlive the turn it was granted in"
        );
    }

    #[tokio::test]
    async fn session_allow_pattern_skips_prompt() {
        let (action, state) = action_with_store().await;
        state.gate().allow_pattern(COUNTER_RESET, "counter:");
        let out = action.run().await.unwrap();
        assert_eq!(out, r#"{"reset":true}"#);
    }

    #[test]
    fn spec_has_expected_name() {
        assert_eq!(ResetAction::spec().name.as_str(), "counter.reset");
    }
}
