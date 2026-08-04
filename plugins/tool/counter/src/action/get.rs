use crate::provider::CounterState;
use ene_plugin::prelude::*;
use std::sync::Arc;

/// Returns the current counter value for the session.
#[derive(Debug, Clone, Deserialize, JsonSchema, ToolAction)]
#[tool(
    namespace = "counter",
    name = "get",
    summary = "Return the current counter value for the session.",
    description = "Returns the current counter value for the active conversation. Starts at 0 and increases by 1 with each counter.increment call. The value is persisted in the shared database, so it survives across turns of the same session.",
    category = "Utility",
    keywords_primary = "counter, count, state, session",
    side_effects = "ReadOnly"
)]
pub struct GetAction {
    #[tool(skip)]
    #[serde(skip, default = "default_state")]
    state: Arc<CounterState>,
}

impl GetAction {
    /// Creates a new `GetAction`.
    #[must_use]
    pub const fn new(state: Arc<CounterState>) -> Self {
        Self { state }
    }

    async fn run(&self) -> Result<String, ToolError> {
        let session_id = self.state.session_id();
        if session_id.is_empty() {
            return Err(ToolError::internal(
                "counter.get requires an active session".to_string(),
            ));
        }
        let store = self.state.ensure_store().await?;
        let value = store.get(&session_id).await?;
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

    async fn action_with_store() -> (GetAction, Arc<CounterState>) {
        let state = Arc::new(CounterState::new());
        state
            .set_test_store(Arc::new(InMemoryCounterStore::default()))
            .await;
        state.set_session_id("session-1");
        (GetAction::new(state.clone()), state)
    }

    #[tokio::test]
    async fn fresh_session_counts_zero() {
        let (action, _state) = action_with_store().await;
        let out = action.run().await.unwrap();
        assert_eq!(out, r#"{"value":0}"#);
    }

    #[tokio::test]
    async fn reflects_incremented_value() {
        let (action, state) = action_with_store().await;
        state
            .ensure_store()
            .await
            .unwrap()
            .increment("session-1")
            .await
            .unwrap();
        let out = action.run().await.unwrap();
        assert_eq!(out, r#"{"value":1}"#);
    }

    #[tokio::test]
    async fn without_session_is_internal_error() {
        let state = Arc::new(CounterState::new());
        state
            .set_test_store(Arc::new(InMemoryCounterStore::default()))
            .await;
        let action = GetAction::new(state);
        let err = action.run().await.unwrap_err();
        assert!(matches!(
            err,
            ToolError::Generic {
                kind: ene_plugin_proto::ErrorKind::Internal,
                ..
            }
        ));
    }

    #[test]
    fn spec_has_expected_name() {
        let spec = GetAction::spec();
        assert_eq!(spec.name.as_str(), "counter.get");
    }
}
