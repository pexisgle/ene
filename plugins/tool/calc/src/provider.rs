use crate::action;
use async_trait::async_trait;
use ene_plugin::{ActionSetProvider, ToolAction};
use ene_plugin_proto::{ToolError, ToolProvider, ToolSpec};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, RwLock};

/// Configuration for the calc plugin.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(default, rename_all = "snake_case")]
pub struct CalcConfig {
    /// exchangerate.host API access key, used by `calc.currency_convert`.
    /// Falls back to the `EXCHANGERATE_HOST_API_KEY` environment variable.
    pub exchangerate_host_access_key: String,
}

fn generate_calc_schema() -> serde_json::Value {
    let g = schemars::SchemaGenerator::default();
    let schema = g.into_root_schema_for::<CalcConfig>();
    // `schemars::Schema` is a thin wrapper around `serde_json::Value`; the
    // schemas produced by `into_root_schema_for` only contain JSON-safe
    // primitives, so the `Result` from `to_value` is only there for API
    // symmetry. A failure would mean a bug in schemars.
    #[expect(
        clippy::expect_used,
        reason = "schemars::Schema from into_root_schema_for is always JSON-serializable"
    )]
    serde_json::to_value(schema).expect("CalcConfig schema should always serialize")
}

/// Built-in calculation tool provider.
///
/// Exposes `evaluate`, `unit_convert`, `currency_convert`, and
/// `color_convert` via [`ActionSetProvider`]. The exchangerate.host
/// access key is threaded into the currency action through the
/// `set_config` hook; a `RwLock` lets a live reconfigure update the key
/// without restarting the tool binary.
pub struct CalcToolProvider {
    inner: ActionSetProvider,
}

impl CalcToolProvider {
    /// Creates a new `CalcToolProvider`.
    #[must_use]
    pub fn new() -> Self {
        let config = Arc::new(RwLock::new(CalcConfig::default()));
        let actions: Vec<Box<dyn ToolAction>> = vec![
            Box::new(action::EvaluateAction::default()),
            Box::new(action::UnitConvertAction::default()),
            Box::new(action::CurrencyConvertAction::new(config.clone())),
            Box::new(action::ColorConvertAction::default()),
        ];

        let config_hook = config;
        let inner = ActionSetProvider::new(actions)
            .with_config_schema_hook(|| Some(generate_calc_schema()))
            .with_set_config_hook(move |value| {
                match serde_json::from_value::<CalcConfig>(value.clone()) {
                    Ok(cfg) => match config_hook.write() {
                        Ok(mut guard) => *guard = cfg,
                        Err(e) => {
                            tracing::warn!("CalcConfig write lock poisoned: {e}");
                        }
                    },
                    Err(e) => {
                        tracing::warn!("Ignoring malformed calc config: {e}");
                    }
                }
            });

        Self { inner }
    }
}

impl Default for CalcToolProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ToolProvider for CalcToolProvider {
    fn list_specs(&self) -> Vec<ToolSpec> {
        self.inner.list_specs()
    }

    async fn call_tool(&self, name: &str, arguments: &str) -> Result<String, ToolError> {
        self.inner.call_tool(name, arguments).await
    }

    fn set_call_context(&self, ctx: &ene_plugin_proto::CallContext) {
        self.inner.set_call_context(ctx);
    }

    fn set_sandbox(&self, sandbox: &ene_plugin_proto::SandboxConfigData) {
        self.inner.set_sandbox(sandbox);
    }

    fn set_config(&self, config: &serde_json::Value) {
        self.inner.set_config(config);
    }

    fn config_schema(&self) -> Option<serde_json::Value> {
        self.inner.config_schema()
    }
}
