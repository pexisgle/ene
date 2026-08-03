//! Shared result formatting helpers for tool actions.

use ene_plugin_proto::ToolError;

/// Serializes a value to pretty JSON, mapping serialization failures to
/// internal tool errors.
pub(crate) fn ok_json<T: serde::Serialize>(value: &T) -> Result<String, ToolError> {
    serde_json::to_string_pretty(value)
        .map_err(|e| ToolError::internal(format!("json serialization failed: {e}")))
}
