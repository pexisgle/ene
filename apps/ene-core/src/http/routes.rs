use std::fs;
use std::path::PathBuf;
use std::str::FromStr;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::HeaderMap;
use base64::Engine;
use ene_api::{
    AffectView, AnswerJobRequest, AnswerQuestionRequest, ApprovalView, ArtifactView,
    BackupResponse, CharacterView, ClaimResourceRequest, CompactResponse, CreateJobRequest,
    CreateScheduleRequest, CreateSessionRequest, EndSessionRequest, ExclusiveSnapshot, GreetingView,
    Health,
    HistoryResponse, JobView, ListProviderModelsRequest, ListProviderModelsResponse, McpDocument,
    McpServerView, MemoryPatch, MemoryView, MessageMode, MessageRequest, MessageResponse,
    OccupantView, Page, PluginConfigErrorView, PluginConfigField, PluginConfigOptionsView,
    PluginConfigValidateView, PluginConfigValues, PluginConfigView, PluginView, QueuedCancel,
    ResourceKind, RestoreRequest, ScheduleView, SelectGreetingRequest, SelectGreetingResponse,
    SendMessageResponse, SessionPatch, SessionView, SettingsPatch, SoulPatch, SoulSkillsPatch,
    SoulView, SpanView, SplitSessionResponse, StageView, ToolTestRequest, ToolView, UsageView,
};
use ene_body::{InputEffect, VoiceRuntime};
use ene_companion::{
    JournalAction, MemoryId, MemoryScope, avatar_path_for_install, export_dir,
    greeting_options_for_install, import_v3, install_archive, looks_like_package_zip,
    soul_from_install,
};
use ene_kernel::DisplayDepth;
use ene_plane::PopupDecision;
use ene_registry::{Layer, PipelineError};
use ene_session::{
    Block, ClientId, DelegationId, EventKind, EventPayload, NewEvent, NewSession, QuestionId,
    SessionCreatedBy, SessionEndReason, SessionId, SessionKind, SessionMeta, SoulId, Transaction,
    TurnId, v1,
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

pub async fn list_greetings(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Page<GreetingView>>, ApiReject> {
    let soul = parse_soul(&id)?;
    let options = greeting_options_for_soul(&state, soul)?
        .into_iter()
        .map(|(index, text)| GreetingView { index, text })
        .collect();
    Ok(Json(Page::of(options)))
}

fn greeting_options_for_soul(
    state: &AppState,
    soul: SoulId,
) -> Result<Vec<(u32, String)>, ApiReject> {
    let store = state.core.companions();
    let row = store
        .get_soul(soul)
        .map_err(map_companion)?
        .ok_or_else(|| not_found("soul not found"))?;
    let Some((id, version)) = row.character_ref.split_once('@') else {
        return Ok(Vec::new());
    };
    let Some(path) = store.package_path(id, version).map_err(map_companion)? else {
        return Ok(Vec::new());
    };
    greeting_options_for_install(std::path::Path::new(&path)).map_err(map_companion)
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

pub async fn patch_soul_skills(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(patch): Json<SoulSkillsPatch>,
) -> Result<Json<SoulView>, ApiReject> {
    web_mutate_forbidden(&client_id_from_headers(&headers))?;
    let soul = parse_soul(&id)?;
    let refs = normalize_skill_refs(patch.skill_refs)?;
    state
        .core
        .companions()
        .set_skill_refs(soul, &refs)
        .map_err(map_companion)?;
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
            .map(|(soul, body)| {
                let package = state
                    .core
                    .companions()
                    .get_soul(soul)
                    .ok()
                    .flatten()
                    .map(|row| package_fields(&state.core.companions(), &row.character_ref));
                let (package_id, avatar_path) = package.unwrap_or((None, None));
                OccupantView {
                    soul_id: soul.to_string(),
                    body_id: body.map(|id| id.to_string()),
                    package_id,
                    avatar_path,
                }
            })
            .collect(),
    })
}

pub async fn list_sessions(
    State(state): State<AppState>,
    Query(filter): Query<SoulFilter>,
) -> Result<Json<Page<SessionView>>, ApiReject> {
    let idle = state.core.list_idle_sessions().map_err(map_core)?;
    for session in idle {
        finish_session(&state, session, SessionEndReason::IdleTimeout).await?;
    }
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
        finish_session(&state, session, SessionEndReason::Explicit).await?;
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
    finish_session(&state, session, reason).await?;
    get_session(State(state), Path(id)).await
}

/// Abort the running turn, wait until it has committed, write `session/end`, then stop the actor.
async fn finish_session(
    state: &AppState,
    session: SessionId,
    reason: SessionEndReason,
) -> Result<(), ApiReject> {
    state.lanes.stop_turn(session).await.map_err(|err| {
        conflict("lane_busy", "in-flight turn did not stop").with_detail(err.to_string())
    })?;
    state
        .core
        .end_session(session, reason)
        .await
        .map_err(map_core)?;
    state.lanes.close(session).await;
    Ok(())
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
    let response = apply_listen_pcm(&state, session, &req.pcm, req.sample_rate).await?;
    Ok(Json(response))
}

pub(super) async fn apply_listen_pcm(
    state: &AppState,
    session: SessionId,
    pcm: &[f32],
    sample_rate: u32,
) -> Result<SendMessageResponse, ApiReject> {
    let effect = state.core.with_voice(|voice| {
        let effect = voice.push_input(pcm, super::speech::wall_clock_ms());
        super::speech::emit_voice_state(&state.events, voice);
        effect
    });
    match effect {
        InputEffect::BargeIn { .. } => {
            drop(state.core.host().mark_user_speaking(true));
            super::speech::emit_audio_abort(&state.events);
            let lane = state
                .lanes
                .get_or_open(&state.core, session)
                .map_err(map_core)?;
            lane.abort().await.map_err(|err| map_kernel(&err))?;
            Ok(empty_listen())
        }
        InputEffect::NeedsTranscribe => {
            let pcm = state.core.with_voice(VoiceRuntime::take_utterance);
            transcribe_listen(state, session, pcm, sample_rate).await
        }
        InputEffect::Transcript(text) => dispatch_listen_text(state, session, text).await,
        InputEffect::Silence
        | InputEffect::IgnoredDisabled
        | InputEffect::IgnoredSelfVoice
        | InputEffect::Listening
        | InputEffect::HoldForMinSpeech => Ok(empty_listen()),
    }
}

fn empty_listen() -> SendMessageResponse {
    SendMessageResponse {
        turn_id: None,
        entry_id: None,
    }
}

async fn transcribe_listen(
    state: &AppState,
    session: SessionId,
    pcm: Vec<f32>,
    sample_rate: u32,
) -> Result<SendMessageResponse, ApiReject> {
    let binding = state.core.ai().lock().tasks.stt.clone();
    if binding.is_unconfigured() || pcm.is_empty() {
        state.core.with_voice(VoiceRuntime::mark_idle);
        return Ok(empty_listen());
    }
    let result = state
        .core
        .supervisor()
        .transcribe(
            &crate::plugin_profile::task_row_id("stt"),
            ene_plugin_ipc::SttRequest {
                pcm,
                sample_rate: sample_rate.max(1),
                language: None,
                model: binding.model,
                base_url: binding.base_url,
                auth: ene_plugin_ipc::ProviderAuth {
                    api_key: state.core.secret_for("stt"),
                },
            },
        )
        .await
        .map_err(|err| {
            state.core.with_voice(VoiceRuntime::mark_idle);
            bad_request("fault", &err.to_string())
        })?;
    dispatch_listen_text(state, session, result.text).await
}

async fn dispatch_listen_text(
    state: &AppState,
    session: SessionId,
    text: String,
) -> Result<SendMessageResponse, ApiReject> {
    let text = text.trim();
    if text.is_empty() {
        state.core.with_voice(VoiceRuntime::mark_idle);
        return Ok(empty_listen());
    }
    dispatch_message(
        state,
        session,
        &MessageRequest {
            text: text.to_owned(),
            mode: MessageMode::Prompt,
            input_modality: Some("voice".into()),
        },
    )
    .await
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
        if let Some(cached) = state.idem.lock().get(&cache_key) {
            return Ok(Json(cached));
        }
        let response = dispatch_message(&state, session, &req).await?;
        let mut cache = state.idem.lock();
        cache.insert(&cache_key, response.clone());
        return Ok(Json(response));
    }
    Ok(Json(dispatch_message(&state, session, &req).await?))
}

pub async fn select_greeting(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<SelectGreetingRequest>,
) -> Result<Json<SelectGreetingResponse>, ApiReject> {
    let session = parse_session(&id)?;
    let meta = state
        .core
        .store()
        .get_session(session)
        .map_err(map_session)?;
    let text = greeting_options_for_soul(&state, meta.soul_id)?
        .into_iter()
        .find_map(|(index, text)| (index == req.index).then_some(text))
        .ok_or_else(|| bad_request("invalid_message", "unknown greeting"))?;
    let lane = state
        .lanes
        .get_or_open(&state.core, session)
        .map_err(map_core)?;
    let committed = lane
        .record_greeting(&text)
        .await
        .map_err(|err| map_kernel(&err))?;
    Ok(Json(SelectGreetingResponse { committed }))
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

pub async fn create_job(
    State(state): State<AppState>,
    Json(req): Json<CreateJobRequest>,
) -> Result<Json<JobView>, ApiReject> {
    let soul = parse_soul(&req.soul_id)?;
    state
        .core
        .companions()
        .get_soul(soul)
        .map_err(map_companion)?
        .ok_or_else(|| not_found("soul not found"))?;
    let goal = req.goal.trim();
    if goal.is_empty() {
        return Err(bad_request("invalid_message", "goal required"));
    }
    let title = req
        .title
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let value = state
        .core
        .supervisor()
        .registry()
        .execute(
            "delegate.start",
            json!({
                "goal": goal,
                "mode": "public",
                "title": title,
                "soul_id": soul.to_string(),
            }),
            Layer::Surface,
        )
        .await
        .map_err(|err| map_pipeline(&err))?;
    let id = value
        .get("delegation_id")
        .and_then(Value::as_str)
        .ok_or_else(|| bad_request("fault", "job creation returned no id"))?
        .parse::<DelegationId>()
        .map_err(|_| bad_request("fault", "job creation returned an invalid id"))?;
    let row = state
        .core
        .work()
        .get_job(id)
        .map_err(map_work)?
        .ok_or_else(|| bad_request("fault", "job creation returned no job"))?;
    Ok(Json(job_view(row)))
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

pub async fn answer_job(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<AnswerJobRequest>,
) -> Result<Json<JobView>, ApiReject> {
    let job = id
        .parse()
        .map_err(|_| bad_request("invalid_message", "bad job id"))?;
    let host = state.core.host();
    let answered = if req.answers.is_empty() {
        let text = req.text.trim();
        if text.is_empty() {
            return Err(bad_request("invalid_message", "empty answer"));
        }
        let questions = host.answer_all_pending(job, text).map_err(map_work)?;
        questions
            .into_iter()
            .map(|question| (question.question_id(), text.to_owned()))
            .collect::<Vec<_>>()
    } else {
        let answers = req
            .answers
            .iter()
            .map(|answer| answer.trim().to_owned())
            .collect::<Vec<_>>();
        if answers.iter().any(String::is_empty) {
            return Err(bad_request("invalid_message", "empty answer"));
        }
        let questions = host.answer_pending(job, &answers).map_err(map_work)?;
        questions
            .into_iter()
            .zip(answers)
            .map(|(question, answer)| (question.question_id(), answer))
            .collect::<Vec<_>>()
    };
    persist_job_answer(&state, job, &answered).await;
    get_job(State(state), Path(id)).await
}

pub async fn answer_question(
    State(state): State<AppState>,
    Path((id, raw_question_id)): Path<(String, String)>,
    Json(req): Json<AnswerQuestionRequest>,
) -> Result<Json<JobView>, ApiReject> {
    let job = id
        .parse()
        .map_err(|_| bad_request("invalid_message", "bad job id"))?;
    let question_id = raw_question_id
        .parse()
        .map_err(|_| bad_request("invalid_message", "bad question id"))?;
    let text = req.text.trim();
    if text.is_empty() {
        return Err(bad_request("invalid_message", "empty answer"));
    }
    let question = state
        .core
        .host()
        .answer_question(job, question_id, text)
        .map_err(map_work)?;
    persist_job_answer(&state, job, &[(question.question_id(), text.to_owned())]).await;
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
    if patch.completed == Some(true) {
        if row.kind != ene_companion::MemoryKind::Commitment {
            return Err(bad_request(
                "invalid_message",
                "only commitments can be completed",
            ));
        }
        let linked = row.schedule_id.clone();
        state
            .core
            .companions()
            .forget(memory, row.soul_id, JournalAction::Completed)
            .map_err(map_companion)?;
        state.core.disable_linked_schedule(linked.as_deref());
        let updated = state
            .core
            .companions()
            .get_memory(memory)
            .map_err(map_companion)?
            .ok_or_else(|| not_found("memory not found"))?;
        return Ok(Json(memory_view(updated)));
    }
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
    if let Some(schedule_id) = patch.schedule_id.as_deref() {
        if schedule_id.is_empty() {
            state
                .core
                .companions()
                .set_memory_schedule_id(memory, None)
                .map_err(map_companion)?;
        } else if row.kind != ene_companion::MemoryKind::Commitment {
            return Err(bad_request(
                "invalid_message",
                "only commitments can link a schedule",
            ));
        } else {
            let schedule = state
                .core
                .work()
                .get_schedule(schedule_id)
                .map_err(map_work)?
                .ok_or_else(|| not_found("schedule not found"))?;
            if schedule.soul_id != row.soul_id {
                return Err(bad_request(
                    "invalid_message",
                    "schedule soul does not match memory",
                ));
            }
            state
                .core
                .companions()
                .set_memory_schedule_id(memory, Some(schedule_id))
                .map_err(map_companion)?;
        }
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
    let linked = row.schedule_id.clone();
    state
        .core
        .companions()
        .forget(memory, row.soul_id, JournalAction::UserRequest)
        .map_err(map_companion)?;
    state.core.disable_linked_schedule(linked.as_deref());
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
            layer: def.primary_layer().as_str().to_owned(),
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
        .map_err(|err| {
            let class = match &err {
                PipelineError::WrongLayer {
                    required: Layer::Job,
                    ..
                } => "requires_job",
                PipelineError::WrongLayer { .. } => "wrong_layer",
                _ => "failed",
            };
            bad_request(class, &err.to_string())
        })?;
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
            wait_reason: fiber.wait_reason,
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
    let binary = state
        .core
        .supervisor()
        .discover(&row.plugin)
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
        wait_reason: updated.wait_reason,
    }))
}

fn map_plugin_config_errors(
    errors: Vec<ene_plugin_ipc::PluginConfigError>,
) -> Vec<PluginConfigErrorView> {
    errors
        .into_iter()
        .map(|err| PluginConfigErrorView {
            path: err.path,
            message: err.message,
        })
        .collect()
}

pub async fn get_plugin_config(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<PluginConfigView>, ApiReject> {
    let row = state
        .core
        .supervisor()
        .profile_row(&id)
        .ok_or_else(|| not_found("plugin row not found"))?;
    let schema = state
        .core
        .supervisor()
        .plugin_config_schema(&id)
        .await
        .map_err(|err| conflict("plugin_config", &err.to_string()))?;
    let values = if schema.has_config {
        state
            .core
            .supervisor()
            .plugin_config_values(&id, &schema.schema)
    } else {
        json!({})
    };
    Ok(Json(PluginConfigView {
        row_id: row.row_id,
        plugin: row.plugin,
        has_config: schema.has_config,
        schema: schema.schema,
        values,
        secret_keys: schema.secret_keys,
    }))
}

pub async fn validate_plugin_config(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<PluginConfigValues>,
) -> Result<Json<PluginConfigValidateView>, ApiReject> {
    if state.core.supervisor().profile_row(&id).is_none() {
        return Err(not_found("plugin row not found"));
    }
    let result = state
        .core
        .supervisor()
        .plugin_config_validate(&id, body.values)
        .await
        .map_err(|err| conflict("plugin_config", &err.to_string()))?;
    Ok(Json(PluginConfigValidateView {
        ok: result.ok,
        errors: map_plugin_config_errors(result.errors),
        restart_required: result.restart_required,
    }))
}

pub async fn plugin_config_options(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<PluginConfigField>,
) -> Result<Json<PluginConfigOptionsView>, ApiReject> {
    if state.core.supervisor().profile_row(&id).is_none() {
        return Err(not_found("plugin row not found"));
    }
    let result = state
        .core
        .supervisor()
        .plugin_config_options(&id, &body.field)
        .await
        .map_err(|err| conflict("plugin_config", &err.to_string()))?;
    Ok(Json(PluginConfigOptionsView {
        options: result
            .options
            .into_iter()
            .map(|opt| ene_api::PluginConfigOptionView {
                id: opt.id,
                label: opt.label,
            })
            .collect(),
        error: result.error,
        fallback: result.fallback,
    }))
}

pub async fn put_plugin_config(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<PluginConfigValues>,
) -> Result<Json<PluginConfigValidateView>, ApiReject> {
    web_mutate_forbidden(&client_id_from_headers(&headers))?;
    if state.core.supervisor().profile_row(&id).is_none() {
        return Err(not_found("plugin row not found"));
    }
    let schema = state
        .core
        .supervisor()
        .plugin_config_schema(&id)
        .await
        .map_err(|err| conflict("plugin_config", &err.to_string()))?;
    let mut secret_keys = schema.secret_keys;
    if secret_keys.is_empty() {
        secret_keys = ene_plugin_ipc::secret_keys_from_schema(&schema.schema);
    }
    let validated = state
        .core
        .supervisor()
        .plugin_config_validate(&id, body.values.clone())
        .await
        .map_err(|err| conflict("plugin_config", &err.to_string()))?;
    if !validated.ok {
        return Ok(Json(PluginConfigValidateView {
            ok: false,
            errors: map_plugin_config_errors(validated.errors),
            restart_required: false,
        }));
    }
    let result = state
        .core
        .supervisor()
        .plugin_config_apply(&id, body.values.clone())
        .await
        .map_err(|err| conflict("plugin_config", &err.to_string()))?;
    if result.ok {
        crate::plugin_profile::persist_applied_plugin_config(
            state.core.data_dir(),
            state.core.vault(),
            &id,
            &body.values,
            &secret_keys,
        )
        .map_err(|err| conflict("plugin_config", &err.to_string()))?;
    }
    Ok(Json(PluginConfigValidateView {
        ok: result.ok,
        errors: map_plugin_config_errors(result.errors),
        restart_required: result.restart_required,
    }))
}

pub async fn list_provider_models(
    State(state): State<AppState>,
    Json(body): Json<ListProviderModelsRequest>,
) -> Result<Json<ListProviderModelsResponse>, ApiReject> {
    let seam = ene_fiber::task_seam(&body.task)
        .ok_or_else(|| bad_request("invalid_message", "unknown task"))?;
    if ene_fiber::provider_plugin(&body.plugin).is_none() {
        return Err(bad_request("invalid_message", "unknown plugin"));
    }
    let api_key = if body.api_key.is_empty() {
        state.core.secret_for(&body.task)
    } else {
        body.api_key
    };
    tracing::debug!(plugin = %body.plugin, task = %body.task, "list_models");
    let request = ene_plugin_ipc::ListModelsRequest {
        seam: seam.to_owned(),
        base_url: body.base_url,
        auth: ene_plugin_ipc::ProviderAuth { api_key },
    };
    match state
        .core
        .supervisor()
        .list_models(&body.plugin, request)
        .await
    {
        Ok(result) => Ok(Json(ListProviderModelsResponse {
            models: result.models,
            error: result.error,
        })),
        Err(ene_fiber::SupervisorError::UnknownPlugin(_)) => {
            Err(bad_request("invalid_message", "unknown plugin"))
        }
        Err(err) => Ok(Json(ListProviderModelsResponse {
            models: Vec::new(),
            error: Some(err.to_string()),
        })),
    }
}

pub async fn list_provider_assets(
    State(state): State<AppState>,
    Json(body): Json<ene_api::ListProviderAssetsRequest>,
) -> Result<Json<ene_api::ListProviderAssetsResponse>, ApiReject> {
    if ene_fiber::provider_plugin(&body.plugin).is_none() {
        return Err(bad_request("invalid_message", "unknown plugin"));
    }
    let result = state.core.supervisor().list_assets(&body.plugin).await;
    match result {
        Ok(result) => Ok(Json(map_assets_list(result))),
        Err(ene_fiber::SupervisorError::UnknownPlugin(_)) => {
            Err(bad_request("invalid_message", "unknown plugin"))
        }
        Err(err) => Ok(Json(ene_api::ListProviderAssetsResponse {
            assets: Vec::new(),
            error: Some(err.to_string()),
        })),
    }
}

pub async fn install_provider_asset(
    State(state): State<AppState>,
    Json(body): Json<ene_api::InstallProviderAssetRequest>,
) -> Result<Json<ene_api::InstallProviderAssetResponse>, ApiReject> {
    if ene_fiber::provider_plugin(&body.plugin).is_none() {
        return Err(bad_request("invalid_message", "unknown plugin"));
    }
    let request = ene_plugin_ipc::InstallAssetRequest {
        asset_id: body.asset_id,
        version: body.version,
        variant: body.variant,
    };
    match state
        .core
        .supervisor()
        .install_asset(&body.plugin, request)
        .await
    {
        Ok(result) => Ok(Json(ene_api::InstallProviderAssetResponse {
            job_id: result.job_id,
            error: result.error,
        })),
        Err(ene_fiber::SupervisorError::UnknownPlugin(_)) => {
            Err(bad_request("invalid_message", "unknown plugin"))
        }
        Err(err) => Ok(Json(ene_api::InstallProviderAssetResponse {
            job_id: String::new(),
            error: Some(err.to_string()),
        })),
    }
}

pub async fn provider_asset_install_status(
    State(state): State<AppState>,
    Json(body): Json<ene_api::ProviderAssetInstallStatusRequest>,
) -> Result<Json<ene_api::ProviderAssetInstallStatusResponse>, ApiReject> {
    if ene_fiber::provider_plugin(&body.plugin).is_none() {
        return Err(bad_request("invalid_message", "unknown plugin"));
    }
    let request = ene_plugin_ipc::InstallStatusRequest {
        job_id: body.job_id.clone(),
    };
    match state
        .core
        .supervisor()
        .install_asset_status(&body.plugin, request)
        .await
    {
        Ok(result) => Ok(Json(map_install_status(result))),
        Err(ene_fiber::SupervisorError::UnknownPlugin(_)) => {
            Err(bad_request("invalid_message", "unknown plugin"))
        }
        Err(err) => Ok(Json(ene_api::ProviderAssetInstallStatusResponse {
            error: Some(err.to_string()),
            ..ene_api::ProviderAssetInstallStatusResponse::default()
        })),
    }
}

pub async fn set_active_provider_asset(
    State(state): State<AppState>,
    Json(body): Json<ene_api::SetActiveProviderAssetRequest>,
) -> Result<Json<ene_api::SetActiveProviderAssetResponse>, ApiReject> {
    if ene_fiber::provider_plugin(&body.plugin).is_none() {
        return Err(bad_request("invalid_message", "unknown plugin"));
    }
    let request = ene_plugin_ipc::SetActiveAssetRequest {
        asset_id: body.asset_id,
        version: body.version,
    };
    match state
        .core
        .supervisor()
        .set_active_asset(&body.plugin, request)
        .await
    {
        Ok(result) => Ok(Json(ene_api::SetActiveProviderAssetResponse {
            error: result.error,
        })),
        Err(ene_fiber::SupervisorError::UnknownPlugin(_)) => {
            Err(bad_request("invalid_message", "unknown plugin"))
        }
        Err(err) => Ok(Json(ene_api::SetActiveProviderAssetResponse {
            error: Some(err.to_string()),
        })),
    }
}

pub async fn refresh_provider_assets_catalog(
    State(state): State<AppState>,
    Json(body): Json<ene_api::RefreshProviderAssetsCatalogRequest>,
) -> Result<Json<ene_api::RefreshProviderAssetsCatalogResponse>, ApiReject> {
    if ene_fiber::provider_plugin(&body.plugin).is_none() {
        return Err(bad_request("invalid_message", "unknown plugin"));
    }
    match state
        .core
        .supervisor()
        .refresh_asset_catalog(&body.plugin)
        .await
    {
        Ok(_) => Ok(Json(ene_api::RefreshProviderAssetsCatalogResponse {
            refreshed: true,
            error: None,
        })),
        Err(err) => Ok(Json(ene_api::RefreshProviderAssetsCatalogResponse {
            refreshed: false,
            error: Some(err.to_string()),
        })),
    }
}

fn map_assets_list(
    result: ene_plugin_ipc::ListAssetsResult,
) -> ene_api::ListProviderAssetsResponse {
    ene_api::ListProviderAssetsResponse {
        assets: result
            .assets
            .into_iter()
            .map(|asset| ene_api::ProviderAssetView {
                id: asset.id,
                kind: asset.kind,
                label: asset.label,
                description: asset.description,
                recommended: asset.recommended,
                installed: asset.installed,
                active: asset.active,
                active_version: asset.active_version,
                local_path: asset.local_path,
                versions: asset
                    .versions
                    .into_iter()
                    .map(|version| ene_api::ProviderAssetVersionView {
                        version: version.version,
                        size_bytes: version.size_bytes,
                        recommended: version.recommended,
                        installed: version.installed,
                        variant_id: version.variant_id,
                        label: version.label,
                        backend: version.backend,
                        release_tag: version.release_tag,
                    })
                    .collect(),
                seams: asset.seams,
            })
            .collect(),
        error: result.error,
    }
}

fn map_install_status(
    result: ene_plugin_ipc::InstallStatusResult,
) -> ene_api::ProviderAssetInstallStatusResponse {
    ene_api::ProviderAssetInstallStatusResponse {
        phase: result.phase.map(|phase| match phase {
            ene_plugin_ipc::InstallPhase::Pending => ene_api::ProviderAssetInstallPhase::Pending,
            ene_plugin_ipc::InstallPhase::Downloading => {
                ene_api::ProviderAssetInstallPhase::Downloading
            }
            ene_plugin_ipc::InstallPhase::Verifying => {
                ene_api::ProviderAssetInstallPhase::Verifying
            }
            ene_plugin_ipc::InstallPhase::Done => ene_api::ProviderAssetInstallPhase::Done,
            ene_plugin_ipc::InstallPhase::Failed => ene_api::ProviderAssetInstallPhase::Failed,
        }),
        received: result.received,
        total: result.total,
        local_path: result.local_path,
        error: result.error,
    }
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
    let store = state.core.companions();
    let souls = store.list_souls().map_err(map_companion)?;
    let items = store
        .list_packages()
        .map_err(map_companion)?
        .into_iter()
        .map(|(id, version, kind, path)| {
            let character_ref = format!("{id}@{version}");
            let soul_id = souls
                .iter()
                .find(|soul| soul.character_ref == character_ref)
                .map(|soul| soul.id.to_string());
            CharacterView {
                id,
                version,
                kind,
                path,
                soul_id,
            }
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
    let bytes = read_import_bytes(&body, state.core.data_dir(), &state.core.character_home())?;
    let home = state.core.character_home();
    let store = state.core.companions();
    let installed = if looks_like_package_zip(&bytes) {
        install_archive(store.as_ref(), &home, &bytes, 32 * 1024 * 1024).map_err(map_companion)?
    } else {
        import_v3(store.as_ref(), &home, &bytes, 32 * 1024 * 1024).map_err(map_companion)?
    };
    let soul_id = activate_installed_package(&state, &installed)?;
    Ok(Json(CharacterView {
        id: installed.id,
        version: installed.version,
        kind: installed.kind.as_str().to_owned(),
        path: installed.path.display().to_string(),
        soul_id,
    }))
}

pub async fn activate_character(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<CharacterView>, ApiReject> {
    web_mutate_forbidden(&client_id_from_headers(&headers))?;
    let packages = state
        .core
        .companions()
        .list_packages()
        .map_err(map_companion)?;
    let found = packages
        .into_iter()
        .rev()
        .find(|(pkg, _, _, _)| pkg == &id)
        .ok_or_else(|| not_found("character not found"))?;
    let installed = ene_companion::InstalledPackage {
        id: found.0.clone(),
        version: found.1.clone(),
        kind: match found.2.as_str() {
            "soul" => ene_companion::PackageKind::Soul,
            "body" => ene_companion::PackageKind::Body,
            _ => ene_companion::PackageKind::Character,
        },
        path: std::path::PathBuf::from(&found.3),
        digest: String::new(),
        origin_unverified: true,
        warnings: Vec::new(),
    };
    let soul_id = activate_installed_package(&state, &installed)?;
    Ok(Json(CharacterView {
        id: found.0,
        version: found.1,
        kind: found.2,
        path: found.3,
        soul_id,
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
    let plugins = state.core.plugins().lock().clone();
    let mut approval = state.core.approval_settings();
    approval.mode = state.core.plane().mode();
    let mut effective = json!({
        "core": core,
        "harness": state.core.harness(),
        "approval": approval,
        "ai": ai,
        "plugins": plugins,
        "mind": state.core.mind(),
        "store": state.core.store_settings(),
        "body": state.core.body_settings(),
        "voice": state.core.voice_settings(),
        "characters": state.core.character_settings(),
        "ai_chat_key_set": state.core.task_key_set("chat"),
        "ai_classifier_key_set": state.core.task_key_set("classifier"),
        "ai_embedding_key_set": state.core.task_key_set("embedding"),
        "ai_proactive_key_set": state.core.task_key_set("proactive"),
        "ai_tts_key_set": state.core.task_key_set("tts"),
        "ai_stt_key_set": state.core.task_key_set("stt"),
        "ai_approve_key_set": state.core.task_key_set("approve"),
        "ai_job_key_set": state.core.task_key_set("job"),
        "observation_scope": state.core.mind().proactive.world_state.send_scope(),
    });
    let overlay = {
        let settings_path = state.core.data_dir().join("settings.json");
        if settings_path.exists()
            && let Ok(raw) = fs::read_to_string(&settings_path)
            && let Ok(file) = serde_json::from_str::<Value>(&raw)
        {
            file
        } else {
            json!({})
        }
    };
    attach_provider_catalog(&mut effective, state.core.data_dir(), &plugins);
    Json(json!({ "overlay": overlay, "effective": effective }))
}

fn attach_provider_catalog(
    effective: &mut Value,
    data_dir: &std::path::Path,
    plugins: &ene_kernel::PluginSettings,
) {
    let home = plugins.resolved_home(data_dir);
    if let Some(map) = effective.as_object_mut() {
        map.insert(
            "providers".to_owned(),
            Value::Array(ene_fiber::provider_catalog(Some(&home))),
        );
    }
}

pub async fn patch_settings(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(patch): Json<SettingsPatch>,
) -> Result<Json<Value>, ApiReject> {
    web_mutate_forbidden(&client_id_from_headers(&headers))?;
    let allowed = crate::boot::settings_patch_keys();
    if let Some(incoming) = patch.fields.as_object() {
        for key in incoming.keys() {
            if !allowed.iter().any(|allowed| allowed == key) {
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
    take_task_secret(
        &state,
        &patch.fields,
        "/ai/tasks/approve/api_key",
        "ai.approve",
        &mut secrets.approve,
    )?;
    take_task_secret(
        &state,
        &patch.fields,
        "/ai/tasks/job/api_key",
        "ai.job",
        &mut secrets.job,
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
    for task in [
        "chat",
        "classifier",
        "embedding",
        "proactive",
        "tts",
        "stt",
        "approve",
        "job",
    ] {
        if let Some(binding) = current
            .pointer_mut(&format!("/ai/tasks/{task}"))
            .and_then(Value::as_object_mut)
        {
            binding.remove("api_key");
        }
    }
    let body = serde_json::to_string_pretty(&current)
        .map_err(|err| bad_request("fault", &err.to_string()))?;
    ene_config::config::atomic_write(&settings_path, &body)
        .map_err(|err| bad_request("fault", &err.to_string()))?;
    let mut profile_dirty = false;
    if let Some(ai_value) = current.get("ai") {
        let mut ai = state.core.ai().lock().clone();
        crate::overlay_ai(&mut ai, ai_value);
        state.core.replace_ai(ai, secrets);
        profile_dirty = true;
    } else {
        // `replace_ai` locks this mutex; clone first so the guard is not held.
        let ai = state.core.ai().lock().clone();
        state.core.replace_ai(ai, secrets);
    }
    if let Some(plugins_value) = current.get("plugins") {
        let mut plugins = state.core.plugins().lock().clone();
        crate::overlay_plugins(&mut plugins, plugins_value);
        state.core.replace_plugins(plugins);
        profile_dirty = true;
    }
    if profile_dirty {
        state.core.apply_plugin_profile().await;
    }
    if let Some(mind_value) = current.get("mind")
        && let Ok(mind) = serde_json::from_value::<ene_companion::MindSettings>(mind_value.clone())
    {
        state.core.replace_mind(mind);
    }
    if let Some(harness_value) = current.get("harness")
        && let Ok(harness) =
            serde_json::from_value::<ene_kernel::HarnessSettings>(harness_value.clone())
    {
        state.core.replace_harness(harness);
    }
    if let Some(store_value) = current.get("store")
        && let Ok(store) = serde_json::from_value::<ene_session::StoreSettings>(store_value.clone())
    {
        state.core.replace_store_settings(store);
    }
    if let Some(approval_value) = current.get("approval")
        && let Ok(approval) =
            serde_json::from_value::<ene_plane::ApprovalSettings>(approval_value.clone())
    {
        state.core.replace_approval(approval);
    }
    if let Some(body_value) = current.get("body")
        && let Ok(body) = serde_json::from_value::<ene_body::BodySettings>(body_value.clone())
    {
        state.core.replace_body_settings(body);
    }
    if let Some(voice_value) = current.get("voice")
        && let Ok(voice) = serde_json::from_value::<ene_body::VoiceSettings>(voice_value.clone())
    {
        state.core.replace_voice_settings(voice);
    }
    if let Some(characters_value) = current.get("characters")
        && let Ok(characters) =
            serde_json::from_value::<ene_companion::CharacterSettings>(characters_value.clone())
    {
        state.core.replace_character_settings(characters);
    }
    drop(state.core.plane().audit().append("settings", &current));
    if let Some(policy) = current
        .pointer("/core/clients/audio_active_policy")
        .and_then(Value::as_str)
    {
        state.exclusive.set_last_used(policy == "last_used");
    }
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
    let (id, path) = super::backup::backup_now(
        state.core.data_dir(),
        state.core.settings().backup.skills_max_bytes,
    )?;
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
    state.lanes.reset().await;
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
        json!({
            "type": "exclusive.held",
            "resource": resource,
            "client_id": client_id
        }),
    );
    state.events.emit(
        DisplayDepth::Surface,
        json!({
            "type": "notify.hint",
            "title": format!("{resource} claimed"),
            "body": client_id,
            "resource": resource,
            "client_id": client_id
        }),
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
            expires_at: cand.expires_at,
            schedule_id: None,
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
    let (package_id, avatar_path) = package_fields(store, &soul.character_ref);
    SoulView {
        id: soul.id.to_string(),
        character_ref: soul.character_ref,
        display_name,
        body_ref: soul.body_ref.map(|id| id.to_string()),
        voice_ref: soul.voice_ref,
        mood_label: soul.affect.mood_label,
        package_id,
        avatar_path,
        skill_refs: soul.skill_refs,
    }
}

fn package_fields(
    store: &ene_companion::CompanionStore,
    character_ref: &str,
) -> (Option<String>, Option<String>) {
    let Some((id, version)) = character_ref.split_once('@') else {
        return (None, None);
    };
    let Ok(Some(path)) = store.package_path(id, version) else {
        return (None, None);
    };
    let avatar =
        avatar_path_for_install(std::path::Path::new(&path)).map(|path| path.display().to_string());
    (Some(character_ref.to_owned()), avatar)
}

fn activate_installed_package(
    state: &AppState,
    installed: &ene_companion::InstalledPackage,
) -> Result<Option<String>, ApiReject> {
    use ene_companion::PackageKind;
    if installed.kind == PackageKind::Body {
        return Ok(None);
    }
    let character_ref = format!("{}@{}", installed.id, installed.version);
    let store = state.core.companions();
    let existing = store
        .list_souls()
        .map_err(map_companion)?
        .into_iter()
        .find(|soul| soul.character_ref == character_ref);
    let mut soul = if let Some(soul) = existing {
        soul
    } else {
        soul_from_install(store.as_ref(), installed).map_err(map_companion)?
    };
    let has_avatar = avatar_path_for_install(&installed.path).is_some();
    if has_avatar && soul.body_ref.is_none() {
        let body = ene_session::BodyId::new();
        store
            .set_body_ref(soul.id, Some(body))
            .map_err(map_companion)?;
        soul.body_ref = Some(body);
    }
    let catalog = if has_avatar {
        ene_body::BodyCatalog::vrm_default()
    } else {
        ene_body::BodyCatalog::text_default()
    };
    state
        .core
        .present_companion(soul.id, soul.body_ref, catalog)
        .map_err(map_core)?;
    Ok(Some(soul.id.to_string()))
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

fn read_import_bytes(
    body: &Value,
    data_dir: &std::path::Path,
    character_home: &std::path::Path,
) -> Result<Vec<u8>, ApiReject> {
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
        let resolved = resolve_import_path(data_dir, character_home, path)?;
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

fn resolve_import_path(
    data_dir: &std::path::Path,
    character_home: &std::path::Path,
    raw: &str,
) -> Result<PathBuf, ApiReject> {
    if raw.contains("..") {
        return Err(bad_request("invalid_message", "invalid path"));
    }
    let imports = data_dir.join("imports");
    fs::create_dir_all(&imports).map_err(|err| bad_request("fault", &err.to_string()))?;
    fs::create_dir_all(character_home).map_err(|err| bad_request("fault", &err.to_string()))?;
    let allowed = [imports.clone(), character_home.to_path_buf()];
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
        delegation_id: meta.delegation_id.map(|id| id.to_string()),
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
        expires_at: row.expires_at,
        schedule_id: row.schedule_id,
    }
}

fn parse_soul(raw: &str) -> Result<SoulId, ApiReject> {
    SoulId::from_str(raw).map_err(|_| bad_request("invalid_message", "bad soul id"))
}

fn normalize_skill_refs(raw: Vec<String>) -> Result<Vec<String>, ApiReject> {
    let mut out = Vec::new();
    for name in raw {
        let name = name.trim();
        if name.is_empty() {
            continue;
        }
        if name == "." || name == ".." || name.contains('/') || name.contains('\\') {
            return Err(bad_request("invalid_message", "bad skill name"));
        }
        if !out.iter().any(|existing| existing == name) {
            out.push(name.to_owned());
        }
    }
    Ok(out)
}

pub(super) fn parse_session(raw: &str) -> Result<SessionId, ApiReject> {
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

fn map_pipeline(err: &ene_registry::PipelineError) -> ApiReject {
    match &err {
        ene_registry::PipelineError::Denied { .. }
        | ene_registry::PipelineError::Plane(ene_plane::PlaneError::Denied { .. }) => {
            forbidden("job creation denied").with_detail(err.to_string())
        }
        _ => bad_request("fault", &err.to_string()),
    }
}

fn map_work(err: ene_work::WorkError) -> ApiReject {
    match err {
        ene_work::WorkError::UnknownJob(_) | ene_work::WorkError::UnknownSchedule(_) => {
            not_found(&err.to_string())
        }
        ene_work::WorkError::NoOpenQuestion => conflict("question_closed", &err.to_string()),
        ene_work::WorkError::QuestionAnswerCount { .. } => {
            bad_request("invalid_message", &err.to_string())
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
        CoreError::Kernel(err) => map_kernel(&err),
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
        if report.inner_intent.as_deref() == Some("ask_user") {
            let (prompt, questions, question_ids) = ask_user_prompt(state, report);
            let id = report
                .job_id
                .map_or_else(|| report.soul_id.to_string(), |id| id.to_string());
            state.events.emit(
                DisplayDepth::Surface,
                json!({
                    "type": ene_api::QUESTION_ASKED_EVENT,
                    "id": id,
                    "soul_id": report.soul_id.to_string(),
                    "prompt": prompt,
                    "text": prompt,
                    "questions": questions,
                    "question_ids": question_ids,
                }),
            );
        }
        state.events.emit(
            DisplayDepth::Surface,
            json!({
                "type": "job.report",
                "soul_id": report.soul_id.to_string(),
                "job_id": report.job_id.map(|id| id.to_string()),
                "speech": report.speech,
                "inner_intent": report.inner_intent,
                "starts_conversation": report.starts_conversation,
            }),
        );
    }
}

fn ask_user_prompt(
    state: &AppState,
    report: &CompanionReport,
) -> (String, Vec<String>, Vec<String>) {
    let Some(job_id) = report.job_id else {
        return (
            report.speech.clone(),
            vec![report.speech.clone()],
            Vec::new(),
        );
    };
    match state.core.host().combine_pending_questions(job_id) {
        Ok(turn) if !turn.questions.is_empty() => {
            let questions = turn
                .questions
                .iter()
                .map(|question| question.prompt.clone())
                .collect();
            let question_ids = turn
                .questions
                .iter()
                .map(|question| question.question_id().to_string())
                .collect();
            (turn.speech, questions, question_ids)
        }
        _ => (
            report.speech.clone(),
            vec![report.speech.clone()],
            Vec::new(),
        ),
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
    let mut entries = vec![NewEvent::new(
        session.id,
        EventKind::ContextSystemMessage,
        EventPayload::ContextSystemMessage {
            v: v1(),
            blocks: vec![Block::text(report.speech.clone())],
            source_key: "job.report".to_owned(),
        },
    )];
    if report.inner_intent.as_deref() == Some("ask_user")
        && let Some(job_id) = report.job_id
        && let Some(question_id) =
            state
                .core
                .host()
                .open_questions(job_id)
                .ok()
                .and_then(|questions| {
                    questions
                        .iter()
                        .rev()
                        .find(|question| question.prompt == report.speech)
                        .map(|question| {
                            QuestionId::from_mailbox(question.delegation_id, question.mailbox_seq)
                        })
                })
    {
        entries.push(NewEvent::new(
            session.id,
            EventKind::DelegationQuestion,
            EventPayload::DelegationQuestion {
                v: v1(),
                delegation_id: job_id,
                question_id,
                question: report.speech.clone(),
            },
        ));
    }
    drop(
        state
            .core
            .store()
            .commit(Transaction {
                entries,
                usage: Vec::new(),
            })
            .await,
    );
}

async fn persist_job_answer(
    state: &AppState,
    job_id: DelegationId,
    answered: &[(QuestionId, String)],
) {
    if answered.is_empty() {
        return;
    }
    let Ok(job) = state.core.host().status_snapshot(job_id) else {
        return;
    };
    let Ok(sessions) = state.core.store().list_sessions(Some(job.soul_id)) else {
        return;
    };
    let Some(session) = sessions.iter().find(|meta| meta.ended_at.is_none()) else {
        return;
    };
    let mut entries = Vec::with_capacity(answered.len().saturating_mul(2));
    for (question_id, answer_text) in answered {
        if answer_text.is_empty() {
            continue;
        }
        entries.push(NewEvent::new(
            session.id,
            EventKind::UserMessage,
            EventPayload::UserMessage {
                v: v1(),
                turn_id: None,
                blocks: vec![Block::text(answer_text)],
                input_modality: "text".to_owned(),
                client_id: ClientId::new(),
            },
        ));
        entries.push(NewEvent::new(
            session.id,
            EventKind::DelegationAnswer,
            EventPayload::DelegationAnswer {
                v: v1(),
                delegation_id: job_id,
                question_id: *question_id,
            },
        ));
    }
    if entries.is_empty() {
        return;
    }
    drop(
        state
            .core
            .store()
            .commit(Transaction {
                entries,
                usage: Vec::new(),
            })
            .await,
    );
}
