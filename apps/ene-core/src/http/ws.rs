use axum::extract::ws::{Message, WebSocket};
use axum::extract::{Query, State, WebSocketUpgrade};
use axum::http::HeaderMap;
use axum::response::IntoResponse;
use ene_kernel::{DisplayDepth, LiveEvent};
use ene_session::{EventKind, SessionId};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::broadcast;

use super::AppState;
use super::error::{ApiReject, bad_request};

/// Core-level events (approvals, exclusive, jobs) with server-side depth.
#[derive(Clone)]
pub struct CoreBus {
    tx: broadcast::Sender<Envelope>,
}

#[derive(Clone)]
pub(crate) struct Envelope {
    min_depth: DisplayDepth,
    payload: Value,
}

impl CoreBus {
    #[must_use]
    pub fn new(capacity: u32) -> Self {
        let (tx, _) = broadcast::channel(capacity.max(16) as usize);
        Self { tx }
    }

    pub fn emit(&self, min_depth: DisplayDepth, payload: Value) {
        drop(self.tx.send(Envelope { min_depth, payload }));
    }

    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<Envelope> {
        self.tx.subscribe()
    }
}

#[derive(Debug, Deserialize)]
pub struct EventsQuery {
    pub depth: Option<String>,
    pub client_id: Option<String>,
    pub session_id: Option<String>,
    pub cursor: Option<u64>,
    #[expect(
        dead_code,
        reason = "read by the auth middleware from the query string"
    )]
    pub access_token: Option<String>,
}

pub async fn events(
    ws: WebSocketUpgrade,
    headers: HeaderMap,
    Query(query): Query<EventsQuery>,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, ApiReject> {
    let depth = DisplayDepth::parse(query.depth.as_deref().unwrap_or("surface"))
        .map_err(|_| bad_request("invalid_message", "depth must be surface or detail"))?;
    let client_id = query
        .client_id
        .clone()
        .unwrap_or_else(|| "anonymous".to_owned());
    if depth == DisplayDepth::Detail {
        drop(state.core.plane().audit().append(
            "detail_subscribe",
            &json!({ "client_id": client_id, "session_id": query.session_id }),
        ));
    }
    let bearer_proto = headers
        .get("sec-websocket-protocol")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| {
            value
                .split(',')
                .map(str::trim)
                .find(|proto| proto.starts_with("bearer."))
                .map(str::to_owned)
        });
    let upgrade = if let Some(proto) = bearer_proto {
        let static_proto: &'static str = Box::leak(proto.into_boxed_str());
        ws.protocols([static_proto])
    } else {
        ws
    };
    Ok(upgrade.on_upgrade(move |socket| {
        socket_loop(
            socket,
            state,
            depth,
            client_id,
            query.session_id,
            query.cursor,
        )
    }))
}

async fn socket_loop(
    mut socket: WebSocket,
    state: AppState,
    depth: DisplayDepth,
    client_id: String,
    session_id: Option<String>,
    cursor: Option<u64>,
) {
    let mut core_rx = state.events.subscribe();
    let mut live = None;
    if let Some(raw) = session_id.as_deref()
        && let Ok(session) = raw.parse::<SessionId>()
        && let Ok(lane) = state.lanes.get_or_open(&state.core, session)
    {
        if let Some(since) = cursor
            && let Ok(events) = state.core.store().load_events(session, since)
        {
            for event in events {
                if !persist_allowed(depth, &event.kind) {
                    continue;
                }
                let payload = json!({
                    "type": "session.event",
                    "seq": event.seq,
                    "kind": event.kind.as_str(),
                });
                if socket
                    .send(Message::Text(payload.to_string().into()))
                    .await
                    .is_err()
                {
                    state.exclusive.release_client(&client_id);
                    return;
                }
            }
        }
        live = Some(lane.subscribe(depth));
    }

    loop {
        tokio::select! {
            envelope = core_rx.recv() => {
                let Ok(envelope) = envelope else { break };
                if !depth_ok(depth, envelope.min_depth) {
                    continue;
                }
                if socket.send(Message::Text(envelope.payload.to_string().into())).await.is_err() {
                    break;
                }
            }
            live_event = recv_live(&mut live) => {
                let Some(event) = live_event else { continue };
                let payload = live_to_json(&event);
                if socket.send(Message::Text(payload.to_string().into())).await.is_err() {
                    break;
                }
            }
            incoming = socket.recv() => {
                match incoming {
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Ok(Message::Ping(p))) => {
                        if socket.send(Message::Pong(p)).await.is_err() {
                            break;
                        }
                    }
                    Some(Ok(_) | Err(_)) => {}
                }
            }
        }
    }
    state.exclusive.release_client(&client_id);
}

async fn recv_live(live: &mut Option<ene_kernel::LiveSubscription>) -> Option<LiveEvent> {
    match live.as_mut() {
        Some(sub) => sub.recv().await,
        None => std::future::pending::<Option<LiveEvent>>().await,
    }
}

fn depth_ok(sub: DisplayDepth, min: DisplayDepth) -> bool {
    matches!(
        (sub, min),
        (DisplayDepth::Detail, _) | (DisplayDepth::Surface, DisplayDepth::Surface)
    )
}

fn persist_allowed(depth: DisplayDepth, kind: &EventKind) -> bool {
    match kind {
        EventKind::InnerMessage | EventKind::AssistantThinking | EventKind::ToolCall => {
            depth == DisplayDepth::Detail
        }
        _ => true,
    }
}

fn live_to_json(event: &LiveEvent) -> Value {
    match event {
        LiveEvent::TextDelta { turn_id, text } => json!({
            "type": "text.delta",
            "turn_id": turn_id.to_string(),
            "text": text,
        }),
        LiveEvent::InnerMessage { turn_id, text } => json!({
            "type": "inner.message",
            "turn_id": turn_id.map(|id| id.to_string()),
            "text": text,
        }),
        LiveEvent::ThinkingDelta { turn_id, text } => json!({
            "type": "thinking.delta",
            "turn_id": turn_id.to_string(),
            "text": text,
        }),
        LiveEvent::ToolSummary { turn_id, line } => json!({
            "type": "tool.call",
            "turn_id": turn_id.to_string(),
            "summary": line,
        }),
        LiveEvent::ToolDetail {
            turn_id,
            name,
            args,
        } => json!({
            "type": "tool.call",
            "turn_id": turn_id.to_string(),
            "name": name,
            "args": args,
        }),
        LiveEvent::TurnEnded { turn_id, outcome } => json!({
            "type": "session.event",
            "kind": "turn/end",
            "turn_id": turn_id.to_string(),
            "outcome": outcome,
        }),
    }
}
