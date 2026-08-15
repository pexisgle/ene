use crate::provider::CounterState;
use ene_plugin::prelude::*;
use std::sync::Arc;

#[derive(Debug, Clone, Deserialize, JsonSchema, ToolAction)]
#[tool(
    namespace = "counter",
    name = "increment",
    summary = "Increment the session counter and return the new value.",
    description = "Increases the counter for the active conversation by 1 and returns the new value. The counter is stored in the shared database, so the value persists across turns of the same session. This action is intentionally not marked ReadOnly or Idempotent: it mutates state and a retry does not have the same effect, so it is declared unknown and always runs sequentially.",
    category = "Utility",
    keywords_primary = "counter, count, increment, add"
)]
pub struct IncrementAction {
    #[tool(skip)]
    #[serde(skip, default = "default_state")]
    state: Arc<CounterState>,
}

impl IncrementAction {
    #[must_use]
    pub const fn new(state: Arc<CounterState>) -> Self {
        Self { state }
    }

    async fn run(&self) -> Result<String, ToolError> {
        let session_id = self.state.session_id();
        if session_id.is_empty() {
            return Err(ToolError::internal(
                "counter.increment requires an active session".to_string(),
            ));
        }
        let store = self.state.ensure_store().await?;
        let value = store.increment(&session_id).await?;
        Ok(serde_json::json!({ "value": value }).to_string())
    }
}

fn default_state() -> Arc<CounterState> {
    Arc::new(CounterState::new())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::InMemoryCounterStore;

    async fn action_with_store() -> (IncrementAction, Arc<CounterState>) {
        let state = Arc::new(CounterState::new());
        state
            .set_test_store(Arc::new(InMemoryCounterStore::default()))
            .await;
        state.set_session_id("session-1");
        (IncrementAction::new(state.clone()), state)
    }

    #[tokio::test]
    async fn increments_from_zero() {
        let (action, _state) = action_with_store().await;
        let out = action.run().await.unwrap();
        assert_eq!(out, r#"{"value":1}"#);
    }

    #[tokio::test]
    async fn increments_are_stateful_across_calls() {
        let (action, _state) = action_with_store().await;
        action.run().await.unwrap();
        action.run().await.unwrap();
        let out = action.run().await.unwrap();
        assert_eq!(out, r#"{"value":3}"#);
    }

    #[tokio::test]
    async fn malformed_json_is_invalid_arguments() {
        let (action, _state) = action_with_store().await;
        let err = action.execute("not json").await.unwrap_err();
        assert!(matches!(err, ToolError::InvalidArguments { .. }));
    }

    #[test]
    fn spec_has_expected_name() {
        assert_eq!(IncrementAction::spec().name.as_str(), "counter.increment");
    }
}
