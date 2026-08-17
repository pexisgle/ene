use serde_json::Value;

/// Defense-in-depth: drop inner/thinking from surface streams even if the server mis-filters.
pub fn surface_event_allowed(value: &Value) -> bool {
    let event_type = value.get("type").and_then(Value::as_str);
    !matches!(
        event_type,
        Some("inner.message" | "thinking.delta" | "inner.delta")
    )
}

pub fn format_event_line(value: &Value) -> String {
    if let Some(text) = value.get("text").and_then(Value::as_str) {
        let kind = value.get("type").and_then(Value::as_str).unwrap_or("event");
        return format!("{kind}: {text}");
    }
    value.to_string()
}

pub fn surface_history_line(role: &str, text: &str) -> Option<String> {
    if role == "inner" {
        return None;
    }
    Some(format!("{role}: {text}"))
}

/// Occupants first, then remaining souls, de-duplicated (P-107 / P-405).
#[must_use]
pub fn merge_soul_ids(occupants: &[String], extras: &[String]) -> Vec<String> {
    let mut ids = Vec::new();
    for id in occupants.iter().chain(extras) {
        if !id.is_empty() && !ids.contains(id) {
            ids.push(id.clone());
        }
    }
    ids
}
