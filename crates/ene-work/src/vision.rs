use async_trait::async_trait;
use ene_companion::{
    ProactiveObservation, ScreenSummaryStatus, WorldStateMemory, WorldStateSettings,
    WorldStateSnapshot,
};
use ene_plane::Sensitivity;
use ene_registry::{Layer, ToolDefinition, ToolInvoke, ToolRegistry, ToolSource};
use serde_json::{Value, json};
use std::sync::Arc;

/// Dual-path vision (D-16): tool screenshot vs continuous observation.
pub fn register_screenshot_tool(registry: &ToolRegistry, invoke: Arc<dyn ToolInvoke>) {
    registry.register_with(
        ToolDefinition {
            name: "app.screenshot".to_owned(),
            description: "Capture the current screen for the model to look at.".to_owned(),
            parameters: json!({"type":"object","properties":{}}),
            output: json!({"type":"object"}),
            side_effects: Vec::new(),
            source: ToolSource::Harness {
                name: "app".to_owned(),
            },
            timeout_ms: Some(5_000),
            sensitivity: Sensitivity::High,
        },
        invoke,
    );
}

/// Offline screenshot stand-in (no display in the daemon).
#[derive(Debug, Default)]
pub struct PlaceholderScreenshot;

#[async_trait]
impl ToolInvoke for PlaceholderScreenshot {
    async fn invoke(&self, _name: &str, _args: Value) -> Result<Value, String> {
        Ok(json!({ "available": false }))
    }
}

/// Observation summary goes to world-state memory, never the session log.
pub fn observe_screen(
    memory: &mut WorldStateMemory,
    settings: &WorldStateSettings,
    summary: &str,
    idle_seconds: u64,
) -> WorldStateSnapshot {
    let obs = ProactiveObservation {
        captured_at_unix_ms: chrono::Utc::now().timestamp_millis().max(0) as u64,
        activity: None,
        screen_summary: Some(summary.to_owned()),
        screen_summary_status: ScreenSummaryStatus::Available,
    };
    let snap = WorldStateSnapshot::from_observation(&obs, idle_seconds);
    memory.push(snap.clone(), settings);
    snap
}

#[must_use]
pub fn screenshot_is_job_or_surface(registry: &ToolRegistry) -> bool {
    registry.get("app.screenshot").is_some()
        && registry
            .schemas(Layer::Surface)
            .iter()
            .any(|schema| schema.get("name").and_then(Value::as_str) == Some("app.screenshot"))
}
