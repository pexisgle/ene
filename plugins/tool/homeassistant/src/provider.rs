use crate::action;
use crate::approval::ApprovalGate;
use crate::config::HomeAssistantConfig;
use async_trait::async_trait;
use ene_plugin::{ActionSetProvider, ToolAction};
use ene_plugin_proto::{SandboxConfigData, ToolError, ToolProvider, ToolSpec};
use parking_lot::RwLock;
use std::sync::Arc;

/// Shared state for the homeassistant actions.
#[derive(Clone)]
pub struct HomeAssistantState {
    config: Arc<RwLock<HomeAssistantConfig>>,
    client: Arc<reqwest::Client>,
    gate: Arc<ApprovalGate>,
}

impl HomeAssistantState {
    /// Creates a new `HomeAssistantState`.
    #[expect(
        clippy::expect_used,
        reason = "the reqwest client is built from constant configuration, so build cannot fail"
    )]
    #[must_use]
    pub fn new() -> Self {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .user_agent("EneHomeAssistant/0.1")
            // The tools only talk to the user-configured Home Assistant
            // host; a cross-host redirect would forward the Authorization
            // header to an upstream-controlled host, so redirects stay on
            // the original host.
            .redirect(reqwest::redirect::Policy::custom(|attempt| {
                let same_host = match (
                    attempt.previous().last().and_then(|url| url.host_str()),
                    attempt.url().host_str(),
                ) {
                    (Some(previous), Some(next)) => previous == next,
                    _ => false,
                };
                if same_host {
                    attempt.follow()
                } else {
                    attempt.stop()
                }
            }))
            .build()
            .expect("reqwest client builder should not fail");
        Self {
            config: Arc::new(RwLock::new(HomeAssistantConfig::default())),
            client: Arc::new(client),
            gate: Arc::new(ApprovalGate::new()),
        }
    }

    /// Returns the approval gate guarding state-changing actions.
    #[must_use]
    pub fn gate(&self) -> &ApprovalGate {
        &self.gate
    }

    /// Returns the shared HTTP client.
    #[must_use]
    pub fn client(&self) -> &reqwest::Client {
        &self.client
    }

    /// Returns a copy of the current plugin configuration.
    #[must_use]
    pub fn config(&self) -> HomeAssistantConfig {
        self.config.read().clone()
    }

    /// Replaces the plugin configuration, keeping the previous value when
    /// the delivered blob is malformed.
    pub fn set_config(&self, value: &serde_json::Value) {
        match serde_json::from_value::<HomeAssistantConfig>(value.clone()) {
            Ok(config) => *self.config.write() = config,
            Err(e) => tracing::warn!("Ignoring malformed homeassistant config: {e}"),
        }
    }
}

impl Default for HomeAssistantState {
    fn default() -> Self {
        Self::new()
    }
}

/// Built-in Home Assistant tool provider.
///
/// Exposes `homeassistant.state`, `homeassistant.turn_on`,
/// `homeassistant.turn_off`, and `homeassistant.set_temperature` via
/// [`ActionSetProvider`]. The approval gate is threaded through provider
/// hooks so per-turn approvals and session-wide allow patterns survive
/// across the host's post-approval re-invocation.
pub struct HomeAssistantToolProvider {
    inner: ActionSetProvider,
}

impl HomeAssistantToolProvider {
    /// Creates a new `HomeAssistantToolProvider`.
    #[must_use]
    pub fn new() -> Self {
        let state = Arc::new(HomeAssistantState::new());
        let actions: Vec<Box<dyn ToolAction>> = vec![
            Box::new(action::StateAction::new(state.clone())),
            Box::new(action::TurnOnAction::new(state.clone())),
            Box::new(action::TurnOffAction::new(state.clone())),
            Box::new(action::SetTemperatureAction::new(state.clone())),
        ];

        let context_state = state.clone();
        let approve_state = state.clone();
        let allow_state = state.clone();
        let revoke_state = state.clone();
        let config_state = state;
        let inner = ActionSetProvider::new(actions)
            .with_set_call_context_hook(move |conv_id, turn_id| {
                context_state.gate().on_call_context(conv_id, turn_id);
            })
            .with_approve_permission_hook(move |request_id| {
                approve_state.gate().approve_request(request_id);
            })
            .with_allow_pattern_hook(move |action, target_pattern| {
                allow_state.gate().allow_pattern(action, target_pattern);
            })
            .with_revoke_pattern_hook(move |action, target_pattern| {
                revoke_state.gate().revoke_pattern(action, target_pattern);
            })
            .with_set_config_hook(move |value| config_state.set_config(value))
            .with_config_schema_hook(|| Some(crate::config::config_schema()));

        Self { inner }
    }
}

impl Default for HomeAssistantToolProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ToolProvider for HomeAssistantToolProvider {
    fn list_specs(&self) -> Vec<ToolSpec> {
        self.inner.list_specs()
    }

    async fn call_tool(&self, name: &str, arguments: &str) -> Result<String, ToolError> {
        self.inner.call_tool(name, arguments).await
    }

    fn set_config(&self, config: &serde_json::Value) {
        self.inner.set_config(config);
    }

    fn config_schema(&self) -> Option<serde_json::Value> {
        self.inner.config_schema()
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
