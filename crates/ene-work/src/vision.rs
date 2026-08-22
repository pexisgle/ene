//! Dual-path vision: the model calls `app.screenshot` (PNG on the
//! conversation turn); the harness observation path decodes the same payload
//! and summarizes off the session log.

#[cfg(test)]
use async_trait::async_trait;
use base64::Engine;
use ene_companion::{
    ProactiveObservation, ScreenSummaryStatus, WorldStateMemory, WorldStateSettings,
    WorldStateSnapshot,
};
use ene_plane::Sensitivity;
use ene_registry::{Layer, ToolDefinition, ToolInvoke, ToolRegistry, ToolSource};
use serde_json::{Value, json};
use std::sync::Arc;
use thiserror::Error;

/// Register the model-facing `app.screenshot` tool.
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
            category: String::new(),
            keywords: Vec::new(),
            examples: Vec::new(),
            background: false,
        },
        invoke,
    );
}

/// Smallest valid PNG (1×1). Scripted captures use this instead of `{available:false}`.
pub const MINIMAL_PNG: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F, 0x15, 0xC4,
    0x89, 0x00, 0x00, 0x00, 0x0A, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0x00, 0x01, 0x00, 0x00,
    0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE,
    0x42, 0x60, 0x82,
];

/// Failures when turning an `app.screenshot` result into PNG bytes.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ScreenshotError {
    #[error("screenshot unavailable")]
    Unavailable,
    #[error("screenshot missing png_base64")]
    MissingPng,
    #[error("invalid png_base64")]
    InvalidPng,
    #[error("screenshot failed: {0}")]
    Invoke(String),
}

/// Decode a screenshot tool result. `{available: false}` is not a successful look.
///
/// # Errors
///
/// Returns [`ScreenshotError`] when the payload is unavailable, missing, or not Base64.
pub fn screenshot_png(value: &Value) -> Result<Vec<u8>, ScreenshotError> {
    if value.get("available") == Some(&Value::Bool(false)) {
        return Err(ScreenshotError::Unavailable);
    }
    let encoded = value
        .get("png_base64")
        .and_then(Value::as_str)
        .ok_or(ScreenshotError::MissingPng)?;
    let png = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|_| ScreenshotError::InvalidPng)?;
    if png.len() < 8 || !png.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) {
        return Err(ScreenshotError::InvalidPng);
    }
    Ok(png)
}

/// Host observation capture: same `app.screenshot` tool, approval skipped.
///
/// # Errors
///
/// Returns [`ScreenshotError`] when the tool fails or the body is not a PNG.
pub async fn capture_screenshot(registry: &ToolRegistry) -> Result<Vec<u8>, ScreenshotError> {
    let value = registry
        .execute_host("app.screenshot", json!({}))
        .await
        .map_err(|err| ScreenshotError::Invoke(err.to_string()))?;
    screenshot_png(&value)
}

/// Headless stand-in when no display is attached.
#[cfg(test)]
#[derive(Debug, Default)]
pub(crate) struct PlaceholderScreenshot;

#[cfg(test)]
#[async_trait]
impl ToolInvoke for PlaceholderScreenshot {
    async fn invoke(&self, _name: &str, _args: Value) -> Result<Value, String> {
        Ok(json!({ "available": false }))
    }
}

/// In-process PNG capture for tests and scripted observation.
#[cfg(test)]
#[derive(Debug, Clone)]
pub(crate) struct PngScreenshot {
    png: Vec<u8>,
}

#[cfg(test)]
impl PngScreenshot {
    #[must_use]
    pub(crate) fn new(png: impl Into<Vec<u8>>) -> Self {
        Self { png: png.into() }
    }

    #[must_use]
    pub(crate) fn minimal() -> Self {
        Self::new(MINIMAL_PNG)
    }
}

#[cfg(test)]
#[async_trait]
impl ToolInvoke for PngScreenshot {
    async fn invoke(&self, _name: &str, _args: Value) -> Result<Value, String> {
        Ok(json!({
            "png_base64": base64::engine::general_purpose::STANDARD.encode(&self.png),
        }))
    }
}

/// Observation summary goes to world-state memory, never the session log.
pub fn observe_screen(
    memory: &mut WorldStateMemory,
    settings: &WorldStateSettings,
    summary: &str,
    idle_seconds: u64,
) -> WorldStateSnapshot {
    observe_screen_with_activity(memory, settings, summary, idle_seconds, "", "")
}

/// Same as [`observe_screen`], with a privacy-safe window label for the ring.
pub fn observe_screen_with_activity(
    memory: &mut WorldStateMemory,
    settings: &WorldStateSettings,
    summary: &str,
    idle_seconds: u64,
    window_label: &str,
    recent_change: &str,
) -> WorldStateSnapshot {
    let obs = ProactiveObservation {
        captured_at_unix_ms: u64::try_from(chrono::Utc::now().timestamp_millis().max(0))
            .unwrap_or(0),
        activity: Some(ene_companion::ActivitySnapshot {
            idle_seconds: Some(idle_seconds),
            active_window_label: window_label.to_owned(),
            recent_change: recent_change.to_owned(),
        }),
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
