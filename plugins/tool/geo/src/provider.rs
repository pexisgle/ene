use crate::action;
use crate::approval::ApprovalGate;
use async_trait::async_trait;
use ene_plugin::{ActionSetProvider, ToolAction};
use ene_plugin_proto::{SandboxConfigData, ToolError, ToolProvider, ToolSpec};
use std::sync::Arc;

/// Shared state for the geo actions.
#[derive(Clone)]
pub struct GeoState {
    gate: Arc<ApprovalGate>,
    client: Arc<reqwest::Client>,
}

impl GeoState {
    /// Creates a new `GeoState`.
    #[expect(
        clippy::expect_used,
        reason = "the reqwest client is built from constant configuration, so build cannot fail"
    )]
    #[must_use]
    pub fn new() -> Self {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .user_agent("EneGeo/0.1")
            // The tools only trust their fixed API hosts; a cross-host
            // redirect would forward the request and its query parameters
            // to an upstream-controlled host, so redirects stay on the
            // original host.
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
            gate: Arc::new(ApprovalGate::new()),
            client: Arc::new(client),
        }
    }

    /// Returns the approval gate guarding privacy-relevant lookups.
    #[must_use]
    pub fn gate(&self) -> &ApprovalGate {
        &self.gate
    }

    /// Returns the shared HTTP client.
    #[must_use]
    pub fn client(&self) -> &reqwest::Client {
        &self.client
    }
}

impl Default for GeoState {
    fn default() -> Self {
        Self::new()
    }
}

/// Built-in geographic information tool provider.
///
/// Exposes `geo.location`, `geo.weather`, `geo.timezone`, and
/// `geo.sunrise_sunset` via [`ActionSetProvider`]. The approval gate is
/// threaded through provider hooks so per-turn approvals and session-wide
/// allow patterns survive across the host's post-approval re-invocation.
pub struct GeoToolProvider {
    inner: ActionSetProvider,
}

impl GeoToolProvider {
    /// Creates a new `GeoToolProvider`.
    #[must_use]
    pub fn new() -> Self {
        let state = Arc::new(GeoState::new());
        let actions: Vec<Box<dyn ToolAction>> = vec![
            Box::new(action::LocationAction::new(state.clone())),
            Box::new(action::WeatherAction::new(state.clone())),
            Box::new(action::TimezoneAction::default()),
            Box::new(action::SunAction::new(state.clone())),
        ];

        let context_state = state.clone();
        let approve_state = state.clone();
        let allow_state = state.clone();
        let revoke_state = state;
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
            });

        Self { inner }
    }
}

impl Default for GeoToolProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ToolProvider for GeoToolProvider {
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
