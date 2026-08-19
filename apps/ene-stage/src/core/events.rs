use std::sync::Arc;
use std::time::Duration;

use ene_api::ApiClient;
use serde_json::Value;
use tracing::warn;

/// Live events from surface and detail WebSocket feeds.
#[derive(Debug, Clone, PartialEq)]
pub enum LiveEvent {
    TextDelta {
        turn_id: String,
        text: String,
    },
    ThinkingDelta {
        text: String,
    },
    InnerMessage {
        text: String,
    },
    ToolCall {
        summary: String,
    },
    SessionEvent {
        kind: String,
        text: String,
    },
    ApprovalAsked {
        id: String,
        tool: String,
        target: String,
    },
    ApprovalResolved {
        id: String,
        decision: String,
    },
    NotifyHint {
        title: String,
        body: String,
    },
    BodyCommand {
        value: Value,
    },
    AudioChunk {
        pcm: Vec<f32>,
        sample_rate: u32,
    },
    AffectState {
        mood_label: String,
        valence: f32,
        arousal: f32,
    },
    JobReport {
        text: String,
    },
    Disconnected,
}

/// Spawn surface and detail event sockets; merged events arrive on the receiver.
pub fn spawn_event_listeners(
    client: &Arc<ApiClient>,
    session_id: &str,
) -> crossbeam_channel::Receiver<LiveEvent> {
    let (tx, rx) = crossbeam_channel::unbounded();
    for depth in ["surface", "detail"] {
        let tx = tx.clone();
        let client = Arc::clone(client);
        let session_id = session_id.to_owned();
        tokio::spawn(async move {
            event_socket_loop(&client, depth, &session_id, &tx).await;
        });
    }
    rx
}

async fn event_socket_loop(
    client: &ApiClient,
    depth: &str,
    session_id: &str,
    tx: &crossbeam_channel::Sender<LiveEvent>,
) {
    loop {
        match client.events(depth, Some(session_id)).await {
            Ok(mut socket) => {
                loop {
                    match socket.recv_json().await {
                        Ok(Some(value)) => {
                            if let Some(event) = parse_live_event(&value)
                                && tx.send(event).is_err()
                            {
                                return;
                            }
                        }
                        Ok(None) => {
                            drop(tx.send(LiveEvent::Disconnected));
                            break;
                        }
                        Err(err) => {
                            warn!(error = %err, depth, "event socket read failed");
                            drop(tx.send(LiveEvent::Disconnected));
                            break;
                        }
                    }
                }
            }
            Err(err) => {
                warn!(error = %err, depth, "event socket connect failed");
                drop(tx.send(LiveEvent::Disconnected));
            }
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

fn parse_live_event(value: &Value) -> Option<LiveEvent> {
    let event_type = value.get("type")?.as_str()?;
    if event_type.starts_with("body.") {
        return Some(LiveEvent::BodyCommand {
            value: value.clone(),
        });
    }

    match event_type {
        "text.delta" => Some(LiveEvent::TextDelta {
            turn_id: string_field(value, "turn_id"),
            text: string_field(value, "text"),
        }),
        "thinking.delta" => Some(LiveEvent::ThinkingDelta {
            text: string_field(value, "text"),
        }),
        "inner.message" | "inner.delta" => Some(LiveEvent::InnerMessage {
            text: string_field(value, "text"),
        }),
        "tool.call" | "tool.progress" | "tool.result" => Some(LiveEvent::ToolCall {
            summary: value
                .get("summary")
                .or_else(|| value.get("line"))
                .or_else(|| value.get("name"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned(),
        }),
        "session.event" => Some(LiveEvent::SessionEvent {
            kind: string_field(value, "kind"),
            text: string_field(value, "text"),
        }),
        "approval.requested" | "approval.asked" => Some(LiveEvent::ApprovalAsked {
            id: string_field(value, "id"),
            tool: string_field(value, "tool"),
            target: string_field(value, "target"),
        }),
        "approval.resolved" => Some(LiveEvent::ApprovalResolved {
            id: string_field(value, "id"),
            decision: string_field(value, "decision"),
        }),
        "notify.hint" => Some(LiveEvent::NotifyHint {
            title: string_field(value, "title"),
            body: value
                .get("body")
                .or_else(|| value.get("text"))
                .or_else(|| value.get("resource"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned(),
        }),
        "audio.chunk" => {
            let pcm = value
                .get("pcm")
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|item| item.as_f64().map(|n| n as f32))
                        .collect::<Vec<f32>>()
                })
                .unwrap_or_default();
            let sample_rate = value
                .get("sample_rate")
                .and_then(Value::as_u64)
                .unwrap_or(44_100) as u32;
            Some(LiveEvent::AudioChunk { pcm, sample_rate })
        }
        "affect.state" => Some(LiveEvent::AffectState {
            mood_label: string_field(value, "mood_label"),
            valence: f32_field(value, "valence"),
            arousal: f32_field(value, "arousal"),
        }),
        "job.report" | "job.progress" | "job.completed" => Some(LiveEvent::JobReport {
            text: value
                .get("speech")
                .or_else(|| value.get("text"))
                .or_else(|| value.get("progress_note"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned(),
        }),
        _ => None,
    }
}

fn string_field(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned()
}

fn f32_field(value: &Value, key: &str) -> f32 {
    value
        .get(key)
        .and_then(Value::as_f64)
        .map_or(0.0, |n| n as f32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_text_delta() {
        let event = parse_live_event(&json!({
            "type": "text.delta",
            "turn_id": "t1",
            "text": "hi"
        }))
        .expect("event");
        assert_eq!(
            event,
            LiveEvent::TextDelta {
                turn_id: "t1".to_owned(),
                text: "hi".to_owned()
            }
        );
    }

    #[test]
    fn parses_body_command_by_prefix() {
        let payload = json!({ "type": "body.expression", "name": "happy" });
        let event = parse_live_event(&payload).expect("event");
        assert!(matches!(event, LiveEvent::BodyCommand { .. }));
    }

    #[test]
    fn parses_approval_requested_alias() {
        let event = parse_live_event(&json!({
            "type": "approval.requested",
            "id": "a1",
            "tool": "fs.write",
            "target": "/tmp/x"
        }))
        .expect("event");
        assert!(matches!(event, LiveEvent::ApprovalAsked { .. }));
    }
}
