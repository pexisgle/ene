use crate::error::SessionError;
use crate::event::{EventKind, LoggedEvent};
use crate::ids::SessionId;
use crate::project::{DisplayDepth, ProjectOptions, ProjectedHistory, derive_messages};
use crate::store::{SessionMeta, SessionStore};
use serde::{Deserialize, Serialize};

const EXPORT_FORMAT_VERSION: u32 = 1;

/// JSON export envelope (redaction applied).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionExport {
    pub format_version: u32,
    pub session_id: SessionId,
    pub soul_id: String,
    pub title: Option<String>,
    pub events: Vec<ExportedEvent>,
}

/// One exported event (payload already redacted by projection rules).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportedEvent {
    pub seq: u64,
    pub ts: String,
    pub kind: String,
    pub payload: serde_json::Value,
}

/// Build JSON + Markdown exports. Child delegation sessions are excluded by the caller.
pub fn export_session(
    meta: &SessionMeta,
    events: &[LoggedEvent],
    include_inner: bool,
    include_thinking: bool,
) -> Result<(SessionExport, String), SessionError> {
    let depth = if include_inner || include_thinking {
        DisplayDepth::Detail
    } else {
        DisplayDepth::Surface
    };
    let mut options = ProjectOptions::for_depth(depth, 8);
    if !include_inner {
        options.inner = crate::project::InnerVisibility::Off;
    }
    if !include_thinking {
        options.thinking = crate::project::ThinkingVisibility::Off;
    }
    let history = derive_messages(events, options);
    let mut exported_events = Vec::new();
    for event in events {
        if !include_event(event, include_inner, include_thinking) {
            continue;
        }
        exported_events.push(ExportedEvent {
            seq: event.seq,
            ts: event.ts.clone(),
            kind: event.kind.as_str().to_owned(),
            payload: scrub_value(serde_json::to_value(&event.payload)?),
        });
    }
    let exported = SessionExport {
        format_version: EXPORT_FORMAT_VERSION,
        session_id: meta.id,
        soul_id: meta.soul_id.to_string(),
        title: meta.title.clone(),
        events: exported_events,
    };
    Ok((exported, history_markdown(&history)))
}

impl SessionStore {
    pub fn export(
        &self,
        session_id: SessionId,
        include_inner: bool,
        include_thinking: bool,
    ) -> Result<(SessionExport, String), SessionError> {
        let meta = self.get_session(session_id)?;
        if matches!(meta.kind, crate::store::SessionKind::Delegation) {
            return Err(SessionError::UnknownSession {
                op: "export",
                session_id: session_id.to_string(),
            });
        }
        let events = self.load_events(session_id, 0)?;
        export_session(&meta, &events, include_inner, include_thinking)
    }
}

fn include_event(event: &LoggedEvent, include_inner: bool, include_thinking: bool) -> bool {
    match event.kind {
        EventKind::InnerMessage => include_inner,
        EventKind::AssistantThinking => include_thinking,
        EventKind::Unknown(_) => false,
        _ => true,
    }
}

fn history_markdown(history: &ProjectedHistory) -> String {
    let mut out = String::from("# Conversation\n\n");
    for message in &history.messages {
        let role = match message.role {
            crate::project::Role::User => "User",
            crate::project::Role::Assistant => "Assistant",
            crate::project::Role::System => "System",
            crate::project::Role::Thinking => "Thinking",
            crate::project::Role::Inner => "Inner",
            crate::project::Role::Tool => "Tool",
            crate::project::Role::Status => "Status",
        };
        out.push_str("## ");
        out.push_str(role);
        out.push_str("\n\n");
        out.push_str(&scrub_text(&message.text()));
        out.push_str("\n\n");
    }
    out
}

fn scrub_text(text: &str) -> String {
    let mut out = text.to_owned();
    for needle in ["sk-", "ghp_", "AKIA"] {
        if let Some(idx) = out.find(needle) {
            let end = (idx + 8).min(out.len());
            out.replace_range(idx..end, "[redacted]");
        }
    }
    out
}

fn scrub_value(mut value: serde_json::Value) -> serde_json::Value {
    if let Some(text) = value.as_str() {
        return serde_json::Value::String(scrub_text(text));
    }
    if let Some(map) = value.as_object_mut() {
        for item in map.values_mut() {
            *item = scrub_value(item.clone());
        }
    }
    if let Some(arr) = value.as_array_mut() {
        for item in arr.iter_mut() {
            *item = scrub_value(item.clone());
        }
    }
    value
}
