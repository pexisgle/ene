use std::fs;
use std::path::PathBuf;
use std::str::FromStr;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::HeaderMap;
use base64::Engine;
use ene_api::{
    AffectView, ApprovalView, ArtifactView, BackupResponse, CharacterView, ClaimResourceRequest,
    CompactResponse, CreateScheduleRequest, CreateSessionRequest, EndSessionRequest,
    ExclusiveSnapshot, Health, HistoryResponse, JobView, McpDocument, McpServerView, MemoryPatch,
    MemoryView, MessageMode, MessageRequest, MessageResponse, OccupantView, Page, PluginView,
    QueuedCancel, ResourceKind, RestoreRequest, ScheduleView, SendMessageResponse, SessionPatch,
    SessionView, SettingsPatch, SoulPatch, SoulView, SpanView, SplitSessionResponse, StageView,
    ToolTestRequest, ToolView, UsageView,
};
use ene_companion::{JournalAction, MemoryId, MemoryScope, export_dir, install_archive};
use ene_fiber::discover_plugin_executable;
use ene_kernel::{DisplayDepth, HarnessSettings};
use ene_plane::PopupDecision;
use ene_registry::Layer;
use ene_session::{
    Block, EventKind, EventPayload, NewEvent, NewSession, SessionCreatedBy, SessionEndReason,
    SessionId, SessionKind, SessionMeta, SoulId, Transaction, TurnId, v1,
};
use ene_work::{CompanionReport, NewSchedule, ScheduleAction};
use serde::Deserialize;
use serde_json::{Value, json};

use super::AppState;
use super::client_id::{client_id_from_headers, web_mutate_forbidden};
use super::error::{ApiReject, bad_request, conflict, forbidden, map_kernel, not_found};
use crate::CoreError;

#[derive(Debug, Deserialize)]
pub struct SoulFilter {
    pub soul_id: Option<String>,
    pub scope: Option<String>,
    pub depth: Option<String>,
    pub session_id: Option<String>,
    pub q: Option<String>,
}

pub async fn health(State(state): State<AppState>) -> Json<Health> {
    Json(Health {
        status: "ok".to_owned(),
        bind: state.bind.to_string(),
    })
}

pub async fn openapi() -> Json<Value> {
    Json(serde_json::from_str(ene_api::openapi_json()).unwrap_or_else(|_| json!({})))
}

pub async fn list_souls(State(state): State<AppState>) -> Result<Json<Page<SoulView>>, ApiReject> {
    let store = state.core.companions();
    let items = store
        .list_souls()
        .map_err(map_companion)?
        .into_iter()
        .map(|soul| soul_view(&store, soul))
        .collect();
    Ok(Json(Page::of(items)))
}

pub async fn get_soul(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<SoulView>, ApiReject> {
    let soul = parse_soul(&id)?;
    let row = state
        .core
        .companions()
        .get_soul(soul)
        .map_err(map_companion)?
        .ok_or_else(|| not_found("soul not found"))?;
    Ok(Json(soul_view(&state.core.companions(), row)))
}

pub async fn patch_soul_body(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(patch): Json<SoulPatch>,
) -> Result<Json<SoulView>, ApiReject> {
    web_mutate_forbidden(&client_id_from_headers(&headers))?;
    let soul = parse_soul(&id)?;
    let body = patch
        .body_ref
        .as_deref()
        .map(|raw| {
            raw.parse()
                .map_err(|_| bad_request("invalid_message", "bad body id"))
        })
        .transpose()?;
    state
        .core
        .companions()
        .set_body_ref(soul, body)
        .map_err(map_companion)?;
    drop(
        state
            .core
            .present_companion(soul, body, ene_body::BodyCatalog::text_default()),
    );
    get_soul(State(state), Path(id)).await
}

pub async fn get_soul_affect(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<AffectView>, ApiReject> {
    let soul = parse_soul(&id)?;
    let row = state
        .core
        .companions()
        .get_soul(soul)
        .map_err(map_companion)?
        .ok_or_else(|| not_found("soul not found"))?;
    Ok(Json(AffectView {
        valence: row.affect.valence,
        arousal: row.affect.arousal,
        dominance: row.affect.dominance,
        trust: row.affect.trust,
        affinity: row.affect.affinity,
        mood_label: row.affect.mood_label,
    }))
}

pub async fn get_stage(State(state): State<AppState>) -> Json<StageView> {
    Json(StageView {
        occupants: state
            .core
            .occupants()
            .into_iter()
            .map(|(soul, body)| OccupantView {
                soul_id: soul.to_string(),
                body_id: body.map(|id| id.to_string()),
            })
            .collect(),
    })
}

pub async fn list_sessions(
    State(state): State<AppState>,
    Query(filter): Query<SoulFilter>,
) -> Result<Json<Page<SessionView>>, ApiReject> {
    state.core.end_idle_sessions().await.map_err(map_core)?;
    let soul = filter.soul_id.as_deref().map(parse_soul).transpose()?;
    let mut items = state
        .core
        .store()
        .list_sessions(soul)
        .map_err(map_session)?;
    if let Some(q) = filter
        .q
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        items.retain(|meta| session_matches_query(&state.core, meta, q));
    }
    Ok(Json(Page::of(
        items.into_iter().map(session_view).collect(),
    )))
}

pub async fn create_session(
    State(state): State<AppState>,
    Json(req): Json<CreateSessionRequest>,
) -> Result<Json<SessionView>, ApiReject> {
    let soul = parse_soul(&req.soul_id)?;
    state
        .core
        .companions()
        .get_soul(soul)
        .map_err(map_companion)?
        .ok_or_else(|| not_found("soul not found"))?;
    let id = state
        .core
        .store()
        .create_session(NewSession {
            soul_id: soul,
            body_id: None,
            kind: SessionKind::Conversation,
            delegation_id: None,
            created_by: SessionCreatedBy::Client,
        })
        .await
        .map_err(map_session)?;
    if let Some(title) = req.title {
        state
            .core
            .store()
            .commit(ene_session::Transaction {
                entries: vec![NewEvent::new(
                    id,
                    EventKind::SessionTitle,
                    EventPayload::SessionTitle { v: v1(), title },
                )],
                usage: Vec::new(),
            })
            .await
            .map_err(map_session)?;
    }
    let meta = state.core.store().get_session(id).map_err(map_session)?;
    Ok(Json(session_view(meta)))
}

pub async fn get_session(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<SessionView>, ApiReject> {
    let session = parse_session(&id)?;
    let meta = state
        .core
        .store()
        .get_session(session)
        .map_err(map_session)?;
    Ok(Json(session_view(meta)))
}

pub async fn patch_session(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(patch): Json<SessionPatch>,
) -> Result<Json<SessionView>, ApiReject> {
    let session = parse_session(&id)?;
    let mut entries = Vec::new();
    if let Some(title) = patch.title {
        entries.push(NewEvent::new(
            session,
            EventKind::SessionTitle,
            EventPayload::SessionTitle { v: v1(), title },
        ));
    }
    if let Some(archived) = patch.archived {
        entries.push(NewEvent::new(
            session,
            EventKind::SessionArchived,
            EventPayload::SessionArchived { v: v1(), archived },
        ));
    }
    if !entries.is_empty() {
        state
            .core
            .store()
            .commit(ene_session::Transaction {
                entries,
                usage: Vec::new(),
            })
            .await
            .map_err(map_session)?;
    }
    get_session(State(state), Path(id)).await
}

pub async fn fork_session(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<SessionView>, ApiReject> {
    let session = parse_session(&id)?;
    let meta = state
        .core
        .store()
        .get_session(session)
        .map_err(map_session)?;
    let boundary = meta.next_seq.saturating_sub(1).max(1);
    let forked = state
        .core
        .store()
        .fork(session, boundary)
        .await
        .map_err(map_session)?;
    let view = state
        .core
        .store()
        .get_session(forked)
        .map_err(map_session)?;
    Ok(Json(session_view(view)))
}

pub async fn split_session(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<SplitSessionResponse>, ApiReject> {
    let session = parse_session(&id)?;
    let previous = state
        .core
        .store()
        .get_session(session)
        .map_err(map_session)?;
    if previous.ended_at.is_none() {
        state
            .core
            .end_session(session, SessionEndReason::Explicit)
            .await
            .map_err(map_core)?;
    }
    let previous = state
        .core
        .store()
        .get_session(session)
        .map_err(map_session)?;
    let new_id = state
        .core
        .store()
        .create_session(NewSession {
            soul_id: previous.soul_id,
            body_id: None,
            kind: SessionKind::Conversation,
            delegation_id: None,
            created_by: SessionCreatedBy::Client,
        })
        .await
        .map_err(map_session)?;
    let created = state
        .core
        .store()
        .get_session(new_id)
        .map_err(map_session)?;
    Ok(Json(SplitSessionResponse {
        previous: session_view(previous),
        session: session_view(created),
    }))
}

pub async fn end_session(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<EndSessionRequest>,
) -> Result<Json<SessionView>, ApiReject> {
    let session = parse_session(&id)?;
    let reason = if req.reason.eq_ignore_ascii_case("idle_timeout") {
        SessionEndReason::IdleTimeout
    } else {
        SessionEndReason::Explicit
    };
    state
        .core
        .end_session(session, reason)
        .await
        .map_err(map_core)?;
    get_session(State(state), Path(id)).await
}

pub async fn barge_in(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiReject> {
    let session = parse_session(&id)?;
    let lane = state
        .lanes
        .get_or_open(&state.core, session)
        .map_err(map_core)?;
    drop(state.core.host().mark_user_speaking(true));
    lane.abort().await.map_err(|err| map_kernel(&err))?;
    Ok(Json(json!({ "ok": true })))
}

pub async fn listen(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<ene_api::ListenRequest>,
) -> Result<Json<SendMessageResponse>, ApiReject> {
    let session = parse_session(&id)?;
    let binding = state.core.ai().lock().tasks.stt.clone();
    if binding.uses_echo() {
        return Ok(Json(SendMessageResponse {
            turn_id: None,
            entry_id: None,
        }));
    }
    let result = state
        .core
        .supervisor()
        .transcribe(
            &binding.plugin,
            ene_plugin_ipc::SttRequest {
                pcm: req.pcm,
                sample_rate: req.sample_rate.max(1),
                language: None,
                model: binding.model,
                base_url: binding.base_url,
                auth: ene_plugin_ipc::ProviderAuth {
                    api_key: state.core.secret_for("stt"),
                },
            },
        )
        .await
        .map_err(|err| bad_request("fault", &err.to_string()))?;
    let text = result.text.trim();
    if text.is_empty() {
        return Ok(Json(SendMessageResponse {
            turn_id: None,
            entry_id: None,
        }));
    }
    dispatch_message(
        &state,
        session,
        &MessageRequest {
            text: text.to_owned(),
            mode: MessageMode::Prompt,
            input_modality: Some("voice".into()),
        },
    )
    .await
    .map(Json)
}

pub async fn export_session(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiReject> {
    let session = parse_session(&id)?;
    let (exported, markdown) = state
        .core
        .store()
        .export(session, false, false)
        .map_err(map_session)?;
    Ok(Json(json!({ "export": exported, "markdown": markdown })))
}

pub async fn send_message(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: axum::http::HeaderMap,
    Json(req): Json<MessageRequest>,
) -> Result<Json<SendMessageResponse>, ApiReject> {
    let session = parse_session(&id)?;
    if let Some(key) = headers
        .get("Idempotency-Key")
        .and_then(|value| value.to_str().ok())
    {
        let cache_key = format!("{id}:{key}");
        if let Some(cached) = state.idem.lock().get(&cache_key).cloned() {
            return Ok(Json(cached));
        }
        let response = dispatch_message(&state, session, &req).await?;
        let mut cache = state.idem.lock();
        if cache.len() >= 256 {
            cache.clear();
        }
        cache.insert(cache_key, response.clone());
        return Ok(Json(response));
    }
    Ok(Json(dispatch_message(&state, session, &req).await?))
}

async fn dispatch_message(
    state: &AppState,
    session: SessionId,
    req: &MessageRequest,
) -> Result<SendMessageResponse, ApiReject> {
    let lane = state
        .lanes
        .get_or_open(&state.core, session)
        .map_err(map_core)?;
    match req.mode {
        MessageMode::Prompt => {
            deliver_speech_gap(state);
            let turn = lane
                .prompt_with_modality(&req.text, req.input_modality.as_deref().unwrap_or("text"))
                .await
                .map_err(|err| map_kernel(&err))?;
            state.lanes.remember_turn(turn, session);
            Ok(SendMessageResponse {
                turn_id: Some(turn.to_string()),
                entry_id: None,
            })
        }
        MessageMode::Steer => {
            let entry = lane
                .steer(&req.text)
                .await
                .map_err(|err| map_kernel(&err))?;
            Ok(SendMessageResponse {
                turn_id: None,
                entry_id: Some(entry),
            })
        }
        MessageMode::FollowUp => {
            let entry = lane
                .follow_up(&req.text)
                .await
                .map_err(|err| map_kernel(&err))?;
            Ok(SendMessageResponse {
                turn_id: None,
                entry_id: Some(entry),
            })
        }
    }
}

pub async fn history(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(filter): Query<SoulFilter>,
) -> Result<Json<HistoryResponse>, ApiReject> {
    let session = parse_session(&id)?;
    let depth = DisplayDepth::parse(filter.depth.as_deref().unwrap_or("surface"))
        .map_err(|_| bad_request("invalid_message", "depth must be surface or detail"))?;
    let lane = state
        .lanes
        .get_or_open(&state.core, session)
        .map_err(map_core)?;
    let projected = lane.project(depth).map_err(|err| map_kernel(&err))?;
    let messages = projected
        .messages
        .iter()
        .map(|message| MessageResponse {
            seq: message.seq,
            role: format!("{:?}", message.role).to_ascii_lowercase(),
            text: message.text(),
        })
        .collect();
    Ok(Json(HistoryResponse {
        messages,
        depth: depth.as_str().to_owned(),
    }))
}

pub async fn cancel_queued(
    State(state): State<AppState>,
    Path((id, entry_id)): Path<(String, u64)>,
) -> Result<Json<QueuedCancel>, ApiReject> {
    let session = parse_session(&id)?;
    let lane = state
        .lanes
        .get_or_open(&state.core, session)
        .map_err(map_core)?;
    let result = lane
        .cancel_queued(entry_id)
        .await
        .map_err(|err| map_kernel(&err))?;
    let name = match result {
        ene_kernel::CancelQueued::Cancelled => "cancelled",
        ene_kernel::CancelQueued::AlreadyConsumed => "already_consumed",
        ene_kernel::CancelQueued::NotFound => "not_found",
    };
    Ok(Json(QueuedCancel {
        result: name.to_owned(),
    }))
}

pub async fn compact(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<CompactResponse>, ApiReject> {
    let session = parse_session(&id)?;
    let lane = state
        .lanes
        .get_or_open(&state.core, session)
        .map_err(map_core)?;
    let entry_id = lane.compact().await.map_err(|err| map_kernel(&err))?;
    Ok(Json(CompactResponse { entry_id }))
}

pub async fn cancel_turn(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiReject> {
    let turn = TurnId::from_str(&id).map_err(|_| bad_request("invalid_message", "bad turn id"))?;
    let session = state
        .lanes
        .session_for_turn(turn)
        .ok_or_else(|| not_found("turn not found"))?;
    let lane = state
        .lanes
        .get_or_open(&state.core, session)
        .map_err(map_core)?;
    lane.abort().await.map_err(|err| map_kernel(&err))?;
    Ok(Json(json!({ "ok": true })))
}

pub async fn list_jobs(
    State(state): State<AppState>,
    Query(filter): Query<SoulFilter>,
) -> Result<Json<Page<JobView>>, ApiReject> {
    let jobs = if let Some(raw) = filter.soul_id.as_deref() {
        let soul = parse_soul(raw)?;
        state.core.work().list_jobs(soul).map_err(map_work)?
    } else {
        state.core.work().list_jobs_all().map_err(map_work)?
    };
    Ok(Json(Page::of(jobs.into_iter().map(job_view).collect())))
}

pub async fn get_job(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<JobView>, ApiReject> {
    let job = id
        .parse()
        .map_err(|_| bad_request("invalid_message", "bad job id"))?;
    let row = state
        .core
        .work()
        .get_job(job)
        .map_err(map_work)?
        .ok_or_else(|| not_found("job not found"))?;
    Ok(Json(job_view(row)))
}

pub async fn cancel_job(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<JobView>, ApiReject> {
    let job = id
        .parse()
        .map_err(|_| bad_request("invalid_message", "bad job id"))?;
    state.core.host().cancel(job).map_err(map_work)?;
    get_job(State(state), Path(id)).await
}

pub async fn list_schedules(
    State(state): State<AppState>,
) -> Result<Json<Page<ScheduleView>>, ApiReject> {
    let items = state
        .core
        .work()
        .list_schedules(None)
        .map_err(map_work)?
        .into_iter()
        .map(schedule_view)
        .collect();
    Ok(Json(Page::of(items)))
}

pub async fn create_schedule(
    State(state): State<AppState>,
    Json(req): Json<CreateScheduleRequest>,
) -> Result<Json<ScheduleView>, ApiReject> {
    let soul = parse_soul(&req.soul_id)?;
    let row = state
        .core
        .work()
        .insert_schedule(&NewSchedule {
            soul_id: soul,
            name: req.name,
            spec: req.spec,
            timezone: req.timezone,
            action: ScheduleAction::parse(&req.action),
            action_ref: req.action_ref,
            important: req.important,
        })
        .map_err(map_work)?;
    Ok(Json(schedule_view(row)))
}

pub async fn patch_schedule(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> Result<Json<ScheduleView>, ApiReject> {
    if let Some(enabled) = body.get("enabled").and_then(Value::as_bool) {
        state
            .core
            .work()
            .set_schedule_enabled(&id, enabled)
            .map_err(map_work)?;
    }
    let row = state
        .core
        .work()
        .get_schedule(&id)
        .map_err(map_work)?
        .ok_or_else(|| not_found("schedule not found"))?;
    Ok(Json(schedule_view(row)))
}

pub async fn delete_schedule(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiReject> {
    state.core.work().delete_schedule(&id).map_err(map_work)?;
    Ok(Json(json!({ "ok": true })))
}

pub async fn list_artifacts(
    State(state): State<AppState>,
    Query(filter): Query<SoulFilter>,
) -> Result<Json<Page<ArtifactView>>, ApiReject> {
    let soul = filter
        .soul_id
        .as_deref()
        .map(parse_soul)
        .transpose()?
        .ok_or_else(|| bad_request("invalid_message", "soul_id required"))?;
    let items = state
        .core
        .work()
        .list_artifacts(soul)
        .map_err(map_work)?
        .into_iter()
        .map(artifact_view)
        .collect();
    Ok(Json(Page::of(items)))
}

pub async fn artifact_content(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiReject> {
    let art = state
        .core
        .work()
        .get_artifact(&id)
        .map_err(map_work)?
        .ok_or_else(|| not_found("artifact not found"))?;
    let confined = ene_registry::confine_tool_path(
        &state.core.workspace_dir(),
        std::path::Path::new(&art.path),
        false,
    )
    .map_err(|_| not_found("artifact not found"))?;
    let text = fs::read_to_string(&confined).unwrap_or_default();
    Ok(Json(
        json!({ "id": art.id, "content": text, "path": confined.display().to_string() }),
    ))
}

pub async fn list_memories(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(filter): Query<SoulFilter>,
) -> Result<Json<Page<MemoryView>>, ApiReject> {
    let soul = parse_soul(&id)?;
    let scope = filter.scope.as_deref().map(MemoryScope::parse);
    let items = state
        .core
        .companions()
        .list_memories(soul, scope)
        .map_err(map_companion)?
        .into_iter()
        .map(memory_view)
        .collect();
    Ok(Json(Page::of(items)))
}

pub async fn patch_memory(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(patch): Json<MemoryPatch>,
) -> Result<Json<MemoryView>, ApiReject> {
    web_mutate_forbidden(&client_id_from_headers(&headers))?;
    let memory =
        MemoryId::from_str(&id).map_err(|_| bad_request("invalid_message", "bad memory id"))?;
    let row = state
        .core
        .companions()
        .get_memory(memory)
        .map_err(map_companion)?
        .ok_or_else(|| not_found("memory not found"))?;
    if let Some(content) = patch.content.as_deref() {
        state
            .core
            .companions()
            .update_memory_content(memory, content, row.soul_id)
            .map_err(map_companion)?;
    }
    if let Some(scope) = patch.scope.as_deref() {
        state
            .core
            .companions()
            .set_scope(memory, MemoryScope::parse(scope), row.soul_id)
            .map_err(map_companion)?;
    }
    let updated = state
        .core
        .companions()
        .get_memory(memory)
        .map_err(map_companion)?
        .ok_or_else(|| not_found("memory not found"))?;
    Ok(Json(memory_view(updated)))
}

pub async fn delete_memory(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiReject> {
    web_mutate_forbidden(&client_id_from_headers(&headers))?;
    let memory =
        MemoryId::from_str(&id).map_err(|_| bad_request("invalid_message", "bad memory id"))?;
    let row = state
        .core
        .companions()
        .get_memory(memory)
        .map_err(map_companion)?
        .ok_or_else(|| not_found("memory not found"))?;
    state
        .core
        .companions()
        .forget(memory, row.soul_id, JournalAction::UserRequest)
        .map_err(map_companion)?;
    drop(
        state
            .core
            .plane()
            .audit()
            .append("memory", &json!({ "action": "forget", "id": id })),
    );
    Ok(Json(json!({ "ok": true })))
}

pub async fn list_tools(State(state): State<AppState>) -> Json<Page<ToolView>> {
    let items = state
        .core
        .supervisor()
        .registry()
        .list()
        .into_iter()
        .map(|def| ToolView {
            layer: if def.surface_visible() {
                "surface".to_owned()
            } else {
                "job".to_owned()
            },
            name: def.name,
            description: def.description,
        })
        .collect();
    Json(Page::of(items))
}

pub async fn test_tool(
    State(state): State<AppState>,
    Path(name): Path<String>,
    headers: HeaderMap,
    Json(req): Json<ToolTestRequest>,
) -> Result<Json<Value>, ApiReject> {
    let client_id = client_id_from_headers(&headers);
    let registry = state.core.supervisor().registry();
    let def = registry
        .list()
        .into_iter()
        .find(|tool| tool.name == name)
        .ok_or_else(|| not_found("tool not found"))?;
    let layer = if def.surface_visible() {
        Layer::Surface
    } else if client_id == "cli" || client_id == "stage" {
        Layer::Job
    } else {
        return Err(forbidden("job-only tools require cli or stage client"));
    };
    let value = registry
        .execute(&name, req.arguments, layer)
        .await
        .map_err(|err| bad_request("failed", &err.to_string()))?;
    Ok(Json(value))
}

pub async fn list_plugins(State(state): State<AppState>) -> Json<Page<PluginView>> {
    let items = state
        .core
        .supervisor()
        .list_fibers()
        .into_iter()
        .map(|fiber| PluginView {
            row_id: fiber.row_id,
            plugin: fiber.plugin,
            state: fiber.state.as_str().to_owned(),
        })
        .collect();
    Json(Page::of(items))
}

pub async fn restart_plugin(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<PluginView>, ApiReject> {
    let row = state
        .core
        .supervisor()
        .profile_row(&id)
        .ok_or_else(|| not_found("plugin row not found"))?;
    let binary = discover_plugin_executable(&row.plugin)
        .ok_or_else(|| conflict("unknown_binary", "plugin binary not found"))?;
    state.core.supervisor().disable_row(&id).await;
    state
        .core
        .supervisor()
        .activate_process(&row, &binary)
        .await
        .map_err(|err| conflict("restart_failed", &err.to_string()))?;
    let updated = state
        .core
        .supervisor()
        .fiber(&id)
        .ok_or_else(|| not_found("plugin row not found"))?;
    Ok(Json(PluginView {
        row_id: updated.row_id,
        plugin: updated.plugin,
        state: updated.state.as_str().to_owned(),
    }))
}

pub async fn get_mcp(State(state): State<AppState>) -> Json<McpDocument> {
    Json(mcp_document(&state.core.mcp_servers()))
}

pub async fn put_mcp(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<McpDocument>,
) -> Result<Json<McpDocument>, ApiReject> {
    web_mutate_forbidden(&client_id_from_headers(&headers))?;
    let mut servers = Vec::with_capacity(body.servers.len());
    for row in &body.servers {
        servers.push(parse_mcp_server(row)?);
    }
    state
        .core
        .replace_mcp_servers(&servers)
        .map_err(|err| bad_request("fault", &err.to_string()))?;
    state.core.apply_plugin_profile().await;
    Ok(Json(mcp_document(&state.core.mcp_servers())))
}

fn mcp_document(servers: &[ene_work::McpServer]) -> McpDocument {
    McpDocument {
        servers: servers.iter().map(mcp_view).collect(),
    }
}

fn mcp_view(server: &ene_work::McpServer) -> McpServerView {
    McpServerView {
        id: server.id.clone(),
        transport: server.transport.clone(),
        command: server.command.clone(),
        args: server.args.clone(),
        url: server.url.clone(),
        enabled: server.enabled,
    }
}

fn parse_mcp_server(row: &McpServerView) -> Result<ene_work::McpServer, ApiReject> {
    if !crate::plugin_profile::valid_mcp_id(&row.id) {
        return Err(bad_request(
            "invalid_message",
            "mcp id must be a short token",
        ));
    }
    let transport = match row.transport.as_str() {
        "" | "stdio" => "stdio",
        "http" | "sse" | "streamable_http" | "streamable-http" => "http",
        _ => {
            return Err(bad_request(
                "invalid_message",
                "mcp transport must be stdio or http",
            ));
        }
    };
    if transport == "http" && row.url.as_deref().unwrap_or("").is_empty() {
        return Err(bad_request("invalid_message", "http MCP rows need url"));
    }
    if transport == "stdio" && row.command.as_deref().unwrap_or("").is_empty() {
        return Err(bad_request(
            "invalid_message",
            "stdio MCP rows need command",
        ));
    }
    Ok(ene_work::McpServer {
        id: row.id.clone(),
        transport: transport.to_owned(),
        command: row.command.clone(),
        args: row.args.clone(),
        url: row.url.clone(),
        enabled: row.enabled,
    })
}

pub async fn list_approvals(State(state): State<AppState>) -> Json<Page<ApprovalView>> {
    let items = state
        .popup
        .list()
        .into_iter()
        .map(|item| ApprovalView {
            id: item.id,
            tool: item.tool,
            target: item.target,
            side_effects: item.side_effects,
        })
        .collect();
    Json(Page::of(items))
}

pub async fn respond_approval(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiReject> {
    web_mutate_forbidden(&client_id_from_headers(&headers))?;
    let decision = body
        .get("decision")
        .and_then(Value::as_str)
        .ok_or_else(|| bad_request("invalid_message", "decision required"))?;
    let parsed = PopupDecision::parse(decision).map_err(|err| match err {
        ene_plane::PlaneError::UnknownApproval(_) => {
            bad_request("invalid_message", "unknown decision")
        }
        other => bad_request("fault", &other.to_string()),
    })?;
    state.popup.respond(&id, parsed).map_err(|err| match err {
        ene_plane::PlaneError::AlreadyResolved(_) => {
            conflict("already_resolved", "approval already resolved")
        }
        ene_plane::PlaneError::UnknownApproval(_) => not_found("approval not found"),
        other => bad_request("fault", &other.to_string()),
    })?;
    state.events.emit(
        DisplayDepth::Surface,
        json!({ "type": "approval.resolved", "id": id, "decision": decision }),
    );
    Ok(Json(json!({ "ok": true })))
}

pub async fn list_characters(
    State(state): State<AppState>,
) -> Result<Json<Page<CharacterView>>, ApiReject> {
    let items = state
        .core
        .companions()
        .list_packages()
        .map_err(map_companion)?
        .into_iter()
        .map(|(id, version, kind, path)| CharacterView {
            id,
            version,
            kind,
            path,
        })
        .collect();
    Ok(Json(Page::of(items)))
}

pub async fn import_character(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Result<Json<CharacterView>, ApiReject> {
    web_mutate_forbidden(&client_id_from_headers(&headers))?;
    let bytes = read_import_bytes(&body, state.core.data_dir())?;
    let home = state.core.data_dir().join("characters");
    let installed = install_archive(
        state.core.companions().as_ref(),
        &home,
        &bytes,
        32 * 1024 * 1024,
    )
    .map_err(map_companion)?;
    Ok(Json(CharacterView {
        id: installed.id,
        version: installed.version,
        kind: installed.kind.as_str().to_owned(),
        path: installed.path.display().to_string(),
    }))
}

pub async fn export_character(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiReject> {
    let packages = state
        .core
        .companions()
        .list_packages()
        .map_err(map_companion)?;
    let found = packages
        .into_iter()
        .find(|(pkg, _, _, _)| pkg == &id)
        .ok_or_else(|| not_found("character not found"))?;
    let zip = export_dir(std::path::Path::new(&found.3)).map_err(map_companion)?;
    let archive_b64 = base64::engine::general_purpose::STANDARD.encode(zip);
    Ok(Json(
        json!({ "id": found.0, "version": found.1, "archive_b64": archive_b64 }),
    ))
}

pub async fn get_settings(State(state): State<AppState>) -> Json<Value> {
    let mut core = state.core.settings().clone();
    core.data_dir = state.core.data_dir().display().to_string();
    let ai = state.core.ai().lock().clone();
    let mut effective = json!({
        "core": core,
        "harness": HarnessSettings::default(),
        "approval": { "mode": format!("{:?}", state.core.plane().mode()).to_ascii_lowercase() },
        "ai": ai,
        "mind": state.core.mind(),
        "ai_chat_key_set": state.core.task_key_set("chat"),
        "ai_classifier_key_set": state.core.task_key_set("classifier"),
        "ai_embedding_key_set": state.core.task_key_set("embedding"),
        "ai_proactive_key_set": state.core.task_key_set("proactive"),
        "ai_tts_key_set": state.core.task_key_set("tts"),
        "ai_stt_key_set": state.core.task_key_set("stt"),
    });
    let mut overlay = json!({});
    let settings_path = state.core.data_dir().join("settings.json");
    if settings_path.exists()
        && let Ok(raw) = fs::read_to_string(&settings_path)
        && let Ok(file) = serde_json::from_str::<Value>(&raw)
    {
        overlay = file;
        deep_merge_objects(effective.as_object_mut(), overlay.as_object());
    }
    Json(json!({ "overlay": overlay, "effective": effective }))
}

pub async fn patch_settings(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(patch): Json<SettingsPatch>,
) -> Result<Json<Value>, ApiReject> {
    web_mutate_forbidden(&client_id_from_headers(&headers))?;
    let allowed = ["core", "harness", "approval", "theme", "ai", "mind"];
    if let Some(incoming) = patch.fields.as_object() {
        for key in incoming.keys() {
            if !allowed.contains(&key.as_str()) {
                return Err(bad_request("invalid_message", "unknown settings key"));
            }
        }
    }
    let mut secrets = crate::TaskSecrets::default();
    take_task_secret(
        &state,
        &patch.fields,
        "/ai/tasks/chat/api_key",
        "ai.chat",
        &mut secrets.chat,
    )?;
    take_task_secret(
        &state,
        &patch.fields,
        "/ai/api_key",
        "ai.chat",
        &mut secrets.chat,
    )?;
    take_task_secret(
        &state,
        &patch.fields,
        "/ai/tasks/classifier/api_key",
        "ai.classifier",
        &mut secrets.classifier,
    )?;
    take_task_secret(
        &state,
        &patch.fields,
        "/ai/tasks/embedding/api_key",
        "ai.embedding",
        &mut secrets.embedding,
    )?;
    take_task_secret(
        &state,
        &patch.fields,
        "/ai/tasks/proactive/api_key",
        "ai.proactive",
        &mut secrets.proactive,
    )?;
    take_task_secret(
        &state,
        &patch.fields,
        "/ai/tasks/tts/api_key",
        "ai.tts",
        &mut secrets.tts,
    )?;
    take_task_secret(
        &state,
        &patch.fields,
        "/ai/tasks/stt/api_key",
        "ai.stt",
        &mut secrets.stt,
    )?;
    let settings_path = state.core.data_dir().join("settings.json");
    let mut current = if settings_path.exists() {
        let raw = fs::read_to_string(&settings_path)
            .map_err(|err| bad_request("fault", &err.to_string()))?;
        serde_json::from_str(&raw).unwrap_or_else(|_| json!({}))
    } else {
        json!({})
    };
    if let (Some(cur), Some(incoming)) = (current.as_object_mut(), patch.fields.as_object()) {
        for (key, value) in incoming {
            if let Some(dst) = cur.get_mut(key) {
                if let (Some(dst_map), Some(src_map)) = (dst.as_object_mut(), value.as_object()) {
                    deep_merge_objects(Some(dst_map), Some(src_map));
                } else {
                    *dst = value.clone();
                }
            } else {
                cur.insert(key.clone(), value.clone());
            }
        }
    }
    for task in ["chat", "classifier", "embedding", "proactive", "tts", "stt"] {
        if let Some(binding) = current
            .pointer_mut(&format!("/ai/tasks/{task}"))
            .and_then(Value::as_object_mut)
        {
            binding.remove("api_key");
        }
    }
    fs::write(&settings_path, current.to_string())
        .map_err(|err| bad_request("fault", &err.to_string()))?;
    if let Some(ai_value) = current.get("ai") {
        let mut ai = state.core.ai().lock().clone();
        crate::overlay_ai(&mut ai, ai_value);
        state.core.replace_ai(ai, secrets);
        state.core.apply_plugin_profile().await;
    } else {
        // `replace_ai` locks this mutex; clone first so the guard is not held.
        let ai = state.core.ai().lock().clone();
        state.core.replace_ai(ai, secrets);
    }
    if let Some(mind_value) = current.get("mind")
        && let Ok(mind) = serde_json::from_value::<ene_companion::MindSettings>(mind_value.clone())
    {
        state.core.replace_mind(mind);
    }
    drop(state.core.plane().audit().append("settings", &current));
    Ok(Json(current))
}

pub async fn settings_schema() -> Json<Value> {
    let raw = match tokio::task::spawn_blocking(ene_config::generate_schema_json).await {
        Ok(Ok(raw)) => raw,
        Ok(Err(_)) | Err(_) => "{}".to_owned(),
    };
    Json(serde_json::from_str(&raw).unwrap_or_else(|_| json!({})))
}

pub async fn audit(State(state): State<AppState>) -> Result<Json<Value>, ApiReject> {
    let records = state
        .core
        .plane()
        .audit()
        .records()
        .map_err(|err| bad_request("fault", &err.to_string()))?;
    Ok(Json(json!({ "items": records })))
}

pub async fn usage(
    State(state): State<AppState>,
    Query(filter): Query<SoulFilter>,
) -> Result<Json<UsageView>, ApiReject> {
    let mut totals = UsageView {
        input_tokens: 0,
        output_tokens: 0,
        cache_read_tokens: 0,
        cache_write_tokens: 0,
        rows: 0,
    };
    let sessions = if let Some(raw) = filter.session_id.as_deref() {
        vec![
            state
                .core
                .store()
                .get_session(parse_session(raw)?)
                .map_err(map_session)?,
        ]
    } else {
        state
            .core
            .store()
            .list_sessions(None)
            .map_err(map_session)?
    };
    for meta in sessions {
        let row = state
            .core
            .store()
            .usage_totals(meta.id)
            .map_err(map_session)?;
        totals.input_tokens += row.input_tokens;
        totals.output_tokens += row.output_tokens;
        totals.cache_read_tokens += row.cache_read_tokens;
        totals.cache_write_tokens += row.cache_write_tokens;
        totals.rows += row.rows;
    }
    Ok(Json(totals))
}

pub async fn diag_spans(State(state): State<AppState>) -> Json<Page<SpanView>> {
    let mut items = Vec::new();
    for lane in state.lanes.all() {
        for span in lane.observe().snapshot() {
            items.push(SpanView {
                name: span.name,
                duration_ms: span.duration.map(|d| d.as_millis()),
                attrs: span.attrs,
            });
        }
    }
    Json(Page::of(items))
}

pub async fn backup(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<BackupResponse>, ApiReject> {
    web_mutate_forbidden(&client_id_from_headers(&headers))?;
    let (id, path) = super::backup::backup_now(state.core.data_dir())?;
    Ok(Json(BackupResponse {
        id,
        path: path.display().to_string(),
    }))
}

pub async fn restore(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<RestoreRequest>,
) -> Result<Json<Value>, ApiReject> {
    web_mutate_forbidden(&client_id_from_headers(&headers))?;
    super::backup::validate_restore_id(&req.id)?;
    let backup_dir = state.core.data_dir().join("backups").join(&req.id);
    if !backup_dir.is_dir() {
        return Err(not_found("backup not found"));
    }
    if state.lanes.any_busy(&state.core) {
        return Err(conflict(
            "lane_busy",
            "cannot restore while a lane is active",
        ));
    }
    let jobs_busy = state.core.work().has_active_jobs().map_err(map_work)?;
    if jobs_busy {
        return Err(conflict(
            "job_busy",
            "cannot restore while a task is running",
        ));
    }
    state
        .core
        .prepare_restore(&req.id)
        .await
        .map_err(|err| bad_request("fault", &err.to_string()))?;
    state
        .core
        .finish_restore()
        .await
        .map_err(|err| bad_request("fault", &err.to_string()))?;
    state.lanes.reset();
    Ok(Json(json!({ "ok": true, "restart_required": true })))
}

pub async fn exclusive_get(State(state): State<AppState>) -> Json<ExclusiveSnapshot> {
    Json(state.exclusive.snapshot())
}

pub async fn exclusive_claim(
    State(state): State<AppState>,
    Path(resource): Path<String>,
    headers: HeaderMap,
    Json(_req): Json<ClaimResourceRequest>,
) -> Result<Json<ExclusiveSnapshot>, ApiReject> {
    let client_id = client_id_from_headers(&headers);
    let kind = ResourceKind::parse(&resource).ok_or_else(|| {
        bad_request(
            "invalid_message",
            "resource must be mic, speaker, or notify",
        )
    })?;
    let snap = state.exclusive.claim(kind, &client_id)?;
    if kind == ResourceKind::Mic {
        drop(state.core.host().mark_user_speaking(true));
    }
    state.events.emit(
        DisplayDepth::Surface,
        json!({ "type": "notify.hint", "resource": resource, "client_id": client_id }),
    );
    Ok(Json(snap))
}

pub async fn exclusive_release(
    State(state): State<AppState>,
    Path(resource): Path<String>,
    headers: HeaderMap,
) -> Result<Json<ExclusiveSnapshot>, ApiReject> {
    let client_id = client_id_from_headers(&headers);
    let kind = ResourceKind::parse(&resource).ok_or_else(|| {
        bad_request(
            "invalid_message",
            "resource must be mic, speaker, or notify",
        )
    })?;
    let held_mic = kind == ResourceKind::Mic && state.exclusive.is_holder(kind, &client_id);
    let snap = state.exclusive.release(kind, &client_id);
    if held_mic {
        deliver_speech_gap(&state);
    }
    Ok(Json(snap))
}

pub async fn list_pending_memories(
    State(state): State<AppState>,
    Query(filter): Query<SoulFilter>,
) -> Result<Json<Page<MemoryView>>, ApiReject> {
    let soul = filter
        .soul_id
        .as_deref()
        .map(parse_soul)
        .transpose()?
        .ok_or_else(|| bad_request("invalid_message", "soul_id required"))?;
    let items = state
        .core
        .companions()
        .list_pending_candidates(soul)
        .map_err(map_companion)?
        .into_iter()
        .map(|cand| MemoryView {
            id: cand.id.to_string(),
            soul_id: cand.soul_id.to_string(),
            scope: cand.scope.as_str().to_owned(),
            kind: cand.kind.as_str().to_owned(),
            title: cand.title,
            content: cand.content,
        })
        .collect();
    Ok(Json(Page::of(items)))
}

#[derive(Debug, Deserialize)]
pub(crate) struct ResolveCandidateBody {
    accept: bool,
}

pub async fn resolve_memory_candidate(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<ResolveCandidateBody>,
) -> Result<Json<Value>, ApiReject> {
    web_mutate_forbidden(&client_id_from_headers(&headers))?;
    let candidate_id = ene_companion::CandidateId::from_str(&id)
        .map_err(|_| bad_request("invalid_message", "bad candidate id"))?;
    let status = if body.accept { "accepted" } else { "rejected" };
    state
        .core
        .companions()
        .resolve_candidate(candidate_id, status)
        .map_err(map_companion)?;
    Ok(Json(json!({ "ok": true, "status": status })))
}

pub async fn web_index() -> axum::response::Html<&'static str> {
    axum::response::Html(include_str!("../../web/index.html"))
}

fn soul_view(store: &ene_companion::CompanionStore, soul: ene_companion::Soul) -> SoulView {
    let display_name = resolve_display_name(store, &soul.character_ref)
        .unwrap_or_else(|| soul.character_ref.clone());
    SoulView {
        id: soul.id.to_string(),
        character_ref: soul.character_ref,
        display_name,
        body_ref: soul.body_ref.map(|id| id.to_string()),
        voice_ref: soul.voice_ref,
        mood_label: soul.affect.mood_label,
    }
}

fn resolve_display_name(
    store: &ene_companion::CompanionStore,
    character_ref: &str,
) -> Option<String> {
    let (id, version) = character_ref.split_once('@')?;
    let path = store.package_path(id, version).ok()??;
    ene_companion::display_name_for_install(std::path::Path::new(&path), "en-US").ok()
}

const MAX_IMPORT_BYTES: u64 = 32 * 1024 * 1024;

fn read_import_bytes(body: &Value, data_dir: &std::path::Path) -> Result<Vec<u8>, ApiReject> {
    if let Some(raw) = body
        .get("archive_b64")
        .or_else(|| body.get("bytes"))
        .and_then(Value::as_str)
    {
        return base64::engine::general_purpose::STANDARD
            .decode(raw)
            .map_err(|err| bad_request("invalid_message", &err.to_string()));
    }
    if let Some(path) = body.get("path").and_then(Value::as_str) {
        let resolved = resolve_import_path(data_dir, path)?;
        let meta = fs::metadata(&resolved).map_err(|err| bad_request("fault", &err.to_string()))?;
        if meta.len() > MAX_IMPORT_BYTES {
            return Err(bad_request("invalid_message", "file too large"));
        }
        return fs::read(&resolved).map_err(|err| bad_request("fault", &err.to_string()));
    }
    Err(bad_request(
        "invalid_message",
        "path or archive_b64 required",
    ))
}

fn resolve_import_path(data_dir: &std::path::Path, raw: &str) -> Result<PathBuf, ApiReject> {
    if raw.contains("..") {
        return Err(bad_request("invalid_message", "invalid path"));
    }
    let imports = data_dir.join("imports");
    fs::create_dir_all(&imports).map_err(|err| bad_request("fault", &err.to_string()))?;
    let allowed = [imports.clone(), data_dir.join("characters")];
    let path = if std::path::Path::new(raw).is_absolute() {
        PathBuf::from(raw)
    } else {
        data_dir.join(raw)
    };
    let canonical =
        fs::canonicalize(&path).map_err(|err| bad_request("fault", &err.to_string()))?;
    for root in allowed {
        let Ok(root_canon) = fs::canonicalize(&root) else {
            continue;
        };
        if canonical.starts_with(&root_canon) {
            return Ok(canonical);
        }
    }
    Err(bad_request("invalid_message", "path outside import dirs"))
}

fn take_task_secret(
    state: &AppState,
    fields: &Value,
    pointer: &str,
    vault_key: &str,
    slot: &mut Option<String>,
) -> Result<(), ApiReject> {
    let Some(value) = fields.pointer(pointer) else {
        return Ok(());
    };
    let Some(secret) = value.as_str() else {
        return Ok(());
    };
    if secret.is_empty() {
        return Ok(());
    }
    state
        .core
        .vault()
        .put(vault_key, secret.as_bytes())
        .map_err(|err| bad_request("fault", &err.to_string()))?;
    *slot = Some(secret.to_owned());
    Ok(())
}

fn deep_merge_objects(
    dst: Option<&mut serde_json::Map<String, Value>>,
    src: Option<&serde_json::Map<String, Value>>,
) {
    let (Some(dst), Some(src)) = (dst, src) else {
        return;
    };
    for (key, value) in src {
        if let (Some(Value::Object(child_dst)), Value::Object(child_src)) =
            (dst.get_mut(key), value)
        {
            deep_merge_objects(Some(child_dst), Some(child_src));
        } else {
            dst.insert(key.clone(), value.clone());
        }
    }
}

fn session_view(meta: ene_session::SessionMeta) -> SessionView {
    SessionView {
        id: meta.id.to_string(),
        soul_id: meta.soul_id.to_string(),
        kind: meta.kind.as_str().to_owned(),
        title: meta.title,
        created_at: meta.created_at,
        archived: meta.archived,
        next_seq: meta.next_seq,
        ended_at: meta.ended_at,
        end_reason: meta.end_reason,
    }
}

fn job_view(job: ene_work::Job) -> JobView {
    JobView {
        id: job.id.to_string(),
        soul_id: job.soul_id.to_string(),
        title: job.title,
        goal: job.goal,
        status: job.status.as_str().to_owned(),
        progress_fraction: job.progress_fraction,
        progress_note: job.progress_note,
    }
}

fn schedule_view(row: ene_work::Schedule) -> ScheduleView {
    ScheduleView {
        id: row.id,
        soul_id: row.soul_id.to_string(),
        name: row.name,
        spec: row.spec,
        timezone: row.timezone,
        action: row.action.as_str().to_owned(),
        enabled: row.enabled,
        important: row.important,
        next_fire: row.next_fire,
    }
}

fn artifact_view(art: ene_work::Artifact) -> ArtifactView {
    ArtifactView {
        id: art.id,
        soul_id: art.soul_id.to_string(),
        title: art.title,
        kind: art.kind.as_str().to_owned(),
        path: art.path,
        delivered: art.delivered,
    }
}

fn memory_view(row: ene_companion::MemoryRecord) -> MemoryView {
    MemoryView {
        id: row.id.to_string(),
        soul_id: row.soul_id.to_string(),
        scope: row.scope.as_str().to_owned(),
        kind: row.kind.as_str().to_owned(),
        title: row.title,
        content: row.content,
    }
}

fn parse_soul(raw: &str) -> Result<SoulId, ApiReject> {
    SoulId::from_str(raw).map_err(|_| bad_request("invalid_message", "bad soul id"))
}

fn parse_session(raw: &str) -> Result<SessionId, ApiReject> {
    SessionId::from_str(raw).map_err(|_| bad_request("invalid_message", "bad session id"))
}

fn session_matches_query(core: &crate::CoreDaemon, meta: &SessionMeta, q: &str) -> bool {
    let needle = q.to_ascii_lowercase();
    if meta.id.to_string().to_ascii_lowercase().contains(&needle) {
        return true;
    }
    if meta
        .title
        .as_deref()
        .unwrap_or("")
        .to_ascii_lowercase()
        .contains(&needle)
    {
        return true;
    }
    let Ok(events) = core.store().load_events(meta.id, 0) else {
        return false;
    };
    events.iter().any(|event| {
        event
            .payload
            .surface_search_text()
            .to_ascii_lowercase()
            .contains(&needle)
    })
}

fn map_session(err: ene_session::SessionError) -> ApiReject {
    match err {
        ene_session::SessionError::SessionNotFound(id) => {
            not_found("session not found").with_detail(id)
        }
        other => bad_request("fault", &other.to_string()),
    }
}

fn map_companion(err: ene_companion::CompanionError) -> ApiReject {
    match err {
        ene_companion::CompanionError::UnknownSoul(_)
        | ene_companion::CompanionError::UnknownMemory(_) => not_found(&err.to_string()),
        other => bad_request("fault", &other.to_string()),
    }
}

fn map_work(err: ene_work::WorkError) -> ApiReject {
    match err {
        ene_work::WorkError::UnknownJob(_) | ene_work::WorkError::UnknownSchedule(_) => {
            not_found(&err.to_string())
        }
        ene_work::WorkError::AlreadyCompleted | ene_work::WorkError::Cancelled => {
            conflict("already_completed", &err.to_string())
        }
        other => bad_request("fault", &other.to_string()),
    }
}

fn map_core(err: CoreError) -> ApiReject {
    match err {
        CoreError::Session(err) => map_session(err),
        other => bad_request("fault", &other.to_string()),
    }
}

pub(crate) fn deliver_speech_gap(state: &AppState) {
    drop(state.core.host().mark_user_speaking(false));
}

pub(crate) fn emit_job_reports(state: &AppState, reports: &[CompanionReport]) {
    for report in reports {
        if report.speech.is_empty() {
            continue;
        }
        state.events.emit(
            DisplayDepth::Surface,
            json!({
                "type": "job.report",
                "soul_id": report.soul_id.to_string(),
                "speech": report.speech,
                "inner_intent": report.inner_intent,
                "starts_conversation": report.starts_conversation,
            }),
        );
    }
}

pub(crate) async fn persist_job_report(state: &AppState, report: &CompanionReport) {
    if report.speech.is_empty() {
        return;
    }
    let Ok(sessions) = state.core.store().list_sessions(Some(report.soul_id)) else {
        return;
    };
    let Some(session) = sessions.iter().find(|meta| meta.ended_at.is_none()) else {
        return;
    };
    drop(
        state
            .core
            .store()
            .commit(Transaction {
                entries: vec![NewEvent::new(
                    session.id,
                    EventKind::ContextSystemMessage,
                    EventPayload::ContextSystemMessage {
                        v: v1(),
                        blocks: vec![Block::text(report.speech.clone())],
                        source_key: "job.report".to_owned(),
                    },
                )],
                usage: Vec::new(),
            })
            .await,
    );
}
