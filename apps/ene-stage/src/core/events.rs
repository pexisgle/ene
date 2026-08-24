use std::sync::Arc;
use std::time::Duration;

use ene_api::ApiClient;
use serde_json::Value;
use tokio::runtime::Handle;
use tracing::warn;

/// Live events from a single WebSocket depth.
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
    SessionNote {
        text: String,
    },
    QuestionAsked {
        id: String,
        prompt: String,
    },
    QuestionResolved {
        id: String,
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
        abort: bool,
        is_final: bool,
        soul_id: Option<String>,
        expression: Option<String>,
    },
    VoiceState {
        state: String,
        barge_in: bool,
    },
    AffectState {
        mood_label: String,
        valence: f32,
        arousal: f32,
    },
    JobReport {
        text: String,
    },
    ExclusiveHeld {
        resource: String,
        client_id: String,
    },
    Disconnected,
}

/// Independent surface and detail sockets. The server filters each feed.
pub struct EventFeeds {
    pub surface: crossbeam_channel::Receiver<LiveEvent>,
    pub detail: crossbeam_channel::Receiver<LiveEvent>,
}

#[cfg(test)]
impl EventFeeds {
    pub(crate) fn new_for_test() -> Self {
        let (surface_tx, surface_rx) = crossbeam_channel::unbounded();
        let (detail_tx, detail_rx) = crossbeam_channel::unbounded();
        drop((surface_tx, detail_tx));
        Self {
            surface: surface_rx,
            detail: detail_rx,
        }
    }
}

/// Spawn one socket per depth. Overlay/chat must only read `surface`.
pub fn spawn_event_feeds(rt: &Handle, client: &Arc<ApiClient>, session_id: &str) -> EventFeeds {
    let (surface_tx, surface_rx) = crossbeam_channel::unbounded();
    let (detail_tx, detail_rx) = crossbeam_channel::unbounded();
    spawn_depth(
        rt,
        Arc::clone(client),
        session_id.to_owned(),
        "surface",
        surface_tx,
    );
    spawn_depth(
        rt,
        Arc::clone(client),
        session_id.to_owned(),
        "detail",
        detail_tx,
    );
    EventFeeds {
        surface: surface_rx,
        detail: detail_rx,
    }
}

fn spawn_depth(
    rt: &Handle,
    client: Arc<ApiClient>,
    session_id: String,
    depth: &'static str,
    tx: crossbeam_channel::Sender<LiveEvent>,
) {
    rt.spawn(async move {
        event_socket_loop(&client, depth, &session_id, &tx).await;
    });
}

async fn event_socket_loop(
    client: &ApiClient,
    depth: &str,
    session_id: &str,
    tx: &crossbeam_channel::Sender<LiveEvent>,
) {
    loop {
        match client.events(depth, Some(session_id)).await {
            Ok(mut socket) => loop {
                match socket.recv_json().await {
                    Ok(Some(value)) => {
                        let event = if depth == "surface" {
                            parse_surface_event(&value)
                        } else {
                            parse_detail_event(&value)
                        };
                        if let Some(event) = event
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
            },
            Err(err) => {
                warn!(error = %err, depth, "event socket connect failed");
                drop(tx.send(LiveEvent::Disconnected));
            }
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

fn parse_surface_event(value: &Value) -> Option<LiveEvent> {
    let event_type = value.get("type")?.as_str()?;
    if event_type.starts_with("inner.")
        || event_type.starts_with("thinking.")
        || event_type == "job.progress"
        || event_type == "affect.state"
        || event_type.starts_with("tool.")
    {
        return None;
    }
    parse_live_event(value)
}

fn parse_detail_event(value: &Value) -> Option<LiveEvent> {
    parse_live_event(value)
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
        "session.event" => {
            let kind = string_field(value, "kind");
            if kind == "tool/denied" {
                return Some(LiveEvent::SessionNote {
                    text: string_field(value, "text"),
                });
            }
            let mut text = string_field(value, "text");
            if text.is_empty() {
                text = string_field(value, "error");
            }
            Some(LiveEvent::SessionEvent { kind, text })
        }
        "approval.requested" | "approval.asked" => Some(LiveEvent::ApprovalAsked {
            id: string_field(value, "id"),
            tool: string_field(value, "tool"),
            target: string_field(value, "target"),
        }),
        "approval.resolved" => Some(LiveEvent::ApprovalResolved {
            id: string_field(value, "id"),
            decision: string_field(value, "decision"),
        }),
        _ if matches!(
            ene_api::QuestionEvent::parse(value).map(|(kind, _)| kind),
            Some(ene_api::QuestionEventKind::Asked)
        ) =>
        {
            ene_api::QuestionEvent::parse(value).map(|(_, event)| LiveEvent::QuestionAsked {
                id: event.id,
                prompt: event.prompt.unwrap_or_default(),
            })
        }
        _ if matches!(
            ene_api::QuestionEvent::parse(value).map(|(kind, _)| kind),
            Some(ene_api::QuestionEventKind::Resolved)
        ) =>
        {
            ene_api::QuestionEvent::parse(value)
                .map(|(_, event)| LiveEvent::QuestionResolved { id: event.id })
        }
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
            let soul_id = value
                .get("soul_id")
                .and_then(Value::as_str)
                .filter(|soul| !soul.is_empty())
                .map(str::to_owned);
            let expression = value
                .get("expression")
                .and_then(Value::as_str)
                .filter(|label| !label.is_empty())
                .map(str::to_owned);
            Some(LiveEvent::AudioChunk {
                pcm,
                sample_rate,
                abort: bool_field(value, "abort"),
                is_final: bool_field(value, "is_final"),
                soul_id,
                expression,
            })
        }
        "voice.state" => Some(LiveEvent::VoiceState {
            state: value
                .get("state")
                .or_else(|| value.get("status"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned(),
            barge_in: value
                .get("barge_in")
                .or_else(|| value.get("bargeIn"))
                .and_then(Value::as_bool)
                .unwrap_or(false),
        }),
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
        "exclusive.held" | "exclusive.changed" => Some(LiveEvent::ExclusiveHeld {
            resource: string_field(value, "resource"),
            client_id: value
                .get("client_id")
                .or_else(|| value.get("holder"))
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

fn bool_field(value: &Value, key: &str) -> bool {
    value.get(key).and_then(Value::as_bool).unwrap_or(false)
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

    #[test]
    fn surface_drops_inner_thinking_pad_and_job_progress() {
        assert!(parse_surface_event(&json!({"type": "thinking.delta", "text": "x"})).is_none());
        assert!(parse_surface_event(&json!({"type": "inner.message", "text": "x"})).is_none());
        assert!(
            parse_surface_event(&json!({"type": "affect.state", "mood_label": "calm"})).is_none()
        );
        assert!(parse_surface_event(&json!({"type": "job.progress", "text": "1"})).is_none());
        assert!(parse_surface_event(&json!({"type": "tool.call", "name": "fs.write"})).is_none());
        assert!(
            parse_surface_event(&json!({"type": "text.delta", "text": "hi", "turn_id": "t"}))
                .is_some()
        );
        assert!(
            parse_surface_event(&json!({"type": "audio.chunk", "pcm": [], "sample_rate": 16000}))
                .is_some()
        );
        assert!(
            parse_surface_event(&json!({"type": "voice.state", "state": "speaking"})).is_some()
        );
        assert!(
            parse_surface_event(&json!({"type": "question.asked", "id": "q", "prompt": "ok?"}))
                .is_some()
        );
        assert!(parse_surface_event(&json!({"type": "question.resolved", "id": "q"})).is_some());
    }

    #[test]
    fn parses_audio_chunk_abort() {
        let event = parse_live_event(&json!({
            "type": "audio.chunk",
            "pcm": [],
            "sample_rate": 16_000,
            "abort": true,
            "is_final": true
        }))
        .expect("event");
        assert_eq!(
            event,
            LiveEvent::AudioChunk {
                pcm: Vec::new(),
                sample_rate: 16_000,
                abort: true,
                is_final: true,
                soul_id: None,
                expression: None,
            }
        );
    }

    #[test]
    fn audio_chunk_abort_defaults_false() {
        let event = parse_live_event(&json!({
            "type": "audio.chunk",
            "pcm": [0.5],
            "sample_rate": 16_000
        }))
        .expect("event");
        assert_eq!(
            event,
            LiveEvent::AudioChunk {
                pcm: vec![0.5],
                sample_rate: 16_000,
                abort: false,
                is_final: false,
                soul_id: None,
                expression: None,
            }
        );
    }

    #[test]
    fn audio_chunk_parses_expression_and_soul() {
        let event = parse_live_event(&json!({
            "type": "audio.chunk",
            "pcm": [0.1],
            "sample_rate": 24_000,
            "soul_id": "01j",
            "expression": "happy"
        }))
        .expect("event");
        assert_eq!(
            event,
            LiveEvent::AudioChunk {
                pcm: vec![0.1],
                sample_rate: 24_000,
                abort: false,
                is_final: false,
                soul_id: Some("01j".to_owned()),
                expression: Some("happy".to_owned()),
            }
        );
    }

    #[test]
    fn detail_keeps_inner_and_thinking() {
        assert!(parse_detail_event(&json!({"type": "thinking.delta", "text": "x"})).is_some());
        assert!(parse_detail_event(&json!({"type": "inner.message", "text": "x"})).is_some());
        assert!(
            parse_detail_event(&json!({"type": "affect.state", "mood_label": "calm"})).is_some()
        );
    }

    #[test]
    fn surface_keeps_turn_end_session_event() {
        let event = parse_surface_event(&json!({
            "type": "session.event",
            "kind": "turn/end",
            "text": ""
        }))
        .expect("event");
        assert!(matches!(
            event,
            LiveEvent::SessionEvent { kind, .. } if kind == "turn/end"
        ));
    }

    #[test]
    fn turn_end_reads_error_when_text_missing() {
        let event = parse_live_event(&json!({
            "type": "session.event",
            "kind": "turn/end",
            "outcome": "failed",
            "error": "model: call failed: 401 Unauthorized"
        }))
        .expect("event");
        assert!(matches!(
            event,
            LiveEvent::SessionEvent { kind, text }
                if kind == "turn/end" && text.contains("401 Unauthorized")
        ));
    }
}
