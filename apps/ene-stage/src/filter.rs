use serde_json::Value;

/// Defense-in-depth: drop inner/thinking from surface streams even if the server mis-filters.
pub fn surface_event_allowed(value: &Value) -> bool {
    let event_type = value.get("type").and_then(Value::as_str);
    if matches!(
        event_type,
        Some("inner.message" | "thinking.delta" | "inner.delta")
    ) {
        return false;
    }
    if event_type == Some("session.event") {
        let kind = value.get("kind").and_then(Value::as_str).unwrap_or("");
        return !matches!(kind, "inner/message" | "assistant/thinking");
    }
    true
}

/// Core-bus `job.report` events are fanned out to every socket; keep them on the owning pane.
#[must_use]
pub fn job_report_matches_soul(value: &Value, soul_id: &str) -> bool {
    if value.get("type").and_then(Value::as_str) != Some("job.report") {
        return true;
    }
    value.get("soul_id").and_then(Value::as_str) == Some(soul_id)
}

pub fn format_event_line(value: &Value) -> String {
    if let Some(text) = value.get("text").and_then(Value::as_str) {
        let kind = value.get("type").and_then(Value::as_str).unwrap_or("event");
        return format!("{kind}: {text}");
    }
    value.to_string()
}

pub fn surface_history_line(role: &str, text: &str) -> Option<String> {
    match role {
        "user" | "assistant" => Some(format!("{role}: {text}")),
        _ => None,
    }
}

#[must_use]
pub fn live_surface_line(value: &Value) -> Option<String> {
    let event_type = value.get("type").and_then(Value::as_str)?;
    match event_type {
        "text.delta" => value
            .get("text")
            .and_then(Value::as_str)
            .map(|t| format!("assistant: {t}")),
        "job.report" => value
            .get("speech")
            .and_then(Value::as_str)
            .filter(|speech| !speech.is_empty())
            .map(|speech| format!("assistant: {speech}")),
        _ => None,
    }
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
