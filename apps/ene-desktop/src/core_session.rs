//! Desktop client session: spawn or attach to `ene-core`, talk HTTP/WS via
//! [`ene_api::ApiClient`], and map live events onto the winit event bus.
#![expect(
    dead_code,
    reason = "session helpers stay for jobs, souls, and schema pages that call them next"
)]
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use ene_api::{
    ApiClient, ApprovalView, CreateScheduleRequest, CreateSessionRequest, HistoryResponse, JobView,
    ListProviderModelsRequest, McpDocument, MemoryPatch, MemoryView, MessageMode, MessageRequest,
    PluginView, ScheduleView, SessionPatch, SessionView, SoulView,
};
use parking_lot::Mutex;
use serde_json::Value;
use tokio::sync::oneshot;

use crate::chat_state::{HistoryEntry, Role};
use crate::core_spawn::{CoreChild, resolve_connection};
use crate::events::{AiStreamUpdate, AppEvent, AppEventSender};
use crate::settings::CoreLifetime;

const BLOCKING_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, thiserror::Error)]
pub enum CoreSessionError {
    #[error("operation timed out after {0}s")]
    Timeout(u64),
    #[error("{0}")]
    Connect(String),
    #[error(transparent)]
    Api(#[from] ene_api::ApiError),
}

pub struct CoreSession {
    client: ApiClient,
    runtime: tokio::runtime::Handle,
    processing: Arc<AtomicBool>,
    active_turn: Arc<Mutex<Option<String>>>,
    soul_id: Arc<Mutex<Option<String>>>,
    session_id: Arc<Mutex<Option<String>>>,
    detail_lines: Arc<Mutex<Vec<String>>>,
    event_tx: AppEventSender,
    core_child: CoreChild,
}

impl CoreSession {
    pub fn try_new(
        event_tx: AppEventSender,
        bootstrap_handle: &tokio::runtime::Handle,
        kill_on_drop: bool,
        audio_tx: Option<crate::audio::AudioChunkSender>,
    ) -> Result<Self, CoreSessionError> {
        let (url, token, core_child) = resolve_connection(bootstrap_handle, kill_on_drop)
            .map_err(CoreSessionError::Connect)?;
        let client = ApiClient::new(url, token, "desktop");
        let processing = Arc::new(AtomicBool::new(false));
        let active_turn = Arc::new(Mutex::new(None));
        let soul_id = Arc::new(Mutex::new(None));
        let session_id = Arc::new(Mutex::new(None));
        let detail_lines = Arc::new(Mutex::new(Vec::new()));
        let session = Self {
            client: client.clone(),
            runtime: bootstrap_handle.clone(),
            processing: processing.clone(),
            active_turn: active_turn.clone(),
            soul_id: soul_id.clone(),
            session_id: session_id.clone(),
            detail_lines: detail_lines.clone(),
            event_tx: event_tx.clone(),
            core_child,
        };
        session.bootstrap()?;
        bootstrap_handle.spawn(pump_events(
            client,
            session.session_id.lock().clone(),
            event_tx,
            processing,
            active_turn,
            detail_lines,
            audio_tx,
        ));
        Ok(session)
    }

    fn bootstrap(&self) -> Result<(), CoreSessionError> {
        let client = self.client.clone();
        let (soul, session) = self.block_on(async move {
            let souls = client.list_souls().await?;
            let soul = souls.items.into_iter().next().ok_or_else(|| {
                ene_api::ApiError::Transport("no companions available".to_owned())
            })?;
            let sessions = client.list_sessions(Some(&soul.id)).await?;
            let session = if let Some(existing) = sessions.items.into_iter().next() {
                existing
            } else {
                client
                    .create_session(&CreateSessionRequest {
                        soul_id: soul.id.clone(),
                        title: None,
                    })
                    .await?
            };
            Ok::<_, ene_api::ApiError>((soul, session))
        })??;
        *self.soul_id.lock() = Some(soul.id);
        *self.session_id.lock() = Some(session.id);
        Ok(())
    }

    pub fn run(&self, input: impl Into<String>) {
        let text = input.into();
        let Some(session_id) = self.session_id.lock().clone() else {
            return;
        };
        self.processing.store(true, Ordering::Relaxed);
        let client = self.client.clone();
        let processing = self.processing.clone();
        let active_turn = self.active_turn.clone();
        self.runtime.spawn(async move {
            let key = uuid::Uuid::new_v4().to_string();
            let result = client
                .send_message(
                    &session_id,
                    &MessageRequest {
                        text,
                        mode: MessageMode::Prompt,
                        input_modality: Some("text".to_owned()),
                    },
                    Some(&key),
                )
                .await;
            match result {
                Ok(response) => {
                    *active_turn.lock() = response.turn_id;
                }
                Err(err) => {
                    tracing::warn!(error = %err, "send_message failed");
                    processing.store(false, Ordering::Relaxed);
                    *active_turn.lock() = None;
                }
            }
        });
    }

    pub fn cancel(&self) {
        let Some(turn_id) = self.active_turn.lock().clone() else {
            return;
        };
        let client = self.client.clone();
        self.runtime.spawn(async move {
            if let Err(err) = client.cancel_turn(&turn_id).await {
                tracing::debug!(error = %err, "cancel_turn failed");
            }
        });
    }

    pub fn barge_in(&self) {
        let Some(session_id) = self.session_id.lock().clone() else {
            return;
        };
        let client = self.client.clone();
        self.runtime.spawn(async move {
            if let Err(err) = client.barge_in(&session_id).await {
                tracing::debug!(error = %err, "barge_in failed");
            }
        });
    }

    pub fn listen(&self, pcm: Vec<f32>, sample_rate: u32) {
        let Some(session_id) = self.session_id.lock().clone() else {
            return;
        };
        if pcm.is_empty() {
            return;
        }
        let client = self.client.clone();
        self.runtime.spawn(async move {
            if let Err(err) = client
                .listen(&session_id, &ene_api::ListenRequest { pcm, sample_rate })
                .await
            {
                tracing::debug!(error = %err, "listen failed");
            }
        });
    }

    pub fn report_beat_pulse(&self, bpm: f32, intensity: f32) {
        drop(self.event_tx.send(AppEvent::BeatPulse { bpm, intensity }));
    }

    #[must_use]
    pub fn is_processing(&self) -> bool {
        self.processing.load(Ordering::Relaxed)
    }

    #[must_use]
    pub fn has_active_turn(&self) -> bool {
        self.active_turn.lock().is_some()
    }

    #[must_use]
    pub fn soul_id(&self) -> Option<String> {
        self.soul_id.lock().clone()
    }

    #[must_use]
    pub fn session_id(&self) -> Option<String> {
        self.session_id.lock().clone()
    }

    #[must_use]
    pub fn client(&self) -> ApiClient {
        self.client.clone()
    }

    #[must_use]
    pub fn bind_label(&self) -> String {
        self.client.base().to_owned()
    }

    pub fn detail_lines(&self) -> Vec<String> {
        self.detail_lines.lock().clone()
    }

    pub fn answer_permission(&self, request_id: String, decision: &str) {
        let client = self.client.clone();
        let decision = decision.to_owned();
        self.runtime.spawn(async move {
            if let Err(err) = client.respond_approval(&request_id, &decision).await {
                tracing::warn!(error = %err, "respond_approval failed");
            }
        });
    }

    pub fn answer_user_input(&self, _request_id: String, text: String) {
        if !text.is_empty() {
            self.run(text);
        }
    }

    pub fn history_blocking(&self) -> Result<Vec<HistoryEntry>, CoreSessionError> {
        let Some(session_id) = self.session_id.lock().clone() else {
            return Ok(Vec::new());
        };
        let client = self.client.clone();
        let history: HistoryResponse =
            self.block_on(async move { client.history(&session_id, "surface").await })??;
        Ok(history
            .messages
            .into_iter()
            .filter_map(|message| {
                Role::from_api(&message.role).map(|role| HistoryEntry {
                    role,
                    content: message.text,
                })
            })
            .collect())
    }

    pub fn settings_blocking(&self) -> Result<Value, CoreSessionError> {
        let client = self.client.clone();
        Ok(self.block_on(async move { client.settings().await })??)
    }

    pub fn settings_schema_blocking(&self) -> Result<Value, CoreSessionError> {
        let client = self.client.clone();
        Ok(self.block_on(async move { client.settings_schema().await })??)
    }

    /// Tokio handle used for host-side background work (e.g. GGUF download).
    #[must_use]
    pub fn runtime_handle(&self) -> &tokio::runtime::Handle {
        &self.runtime
    }

    pub fn spawn_fetch<T: Send + 'static>(
        &self,
        fut: impl std::future::Future<Output = T> + Send + 'static,
    ) -> oneshot::Receiver<T> {
        let (tx, rx) = oneshot::channel();
        self.runtime.spawn(async move {
            let value = fut.await;
            drop(tx.send(value));
        });
        rx
    }

    pub fn fetch_plugins(&self) -> oneshot::Receiver<Vec<PluginView>> {
        let client = self.client.clone();
        self.spawn_fetch(async move {
            client
                .list_plugins()
                .await
                .map(|page| page.items)
                .unwrap_or_default()
        })
    }

    pub fn fetch_sessions(&self) -> oneshot::Receiver<Vec<SessionView>> {
        let client = self.client.clone();
        let soul = self.soul_id.lock().clone();
        self.spawn_fetch(async move {
            client
                .list_sessions(soul.as_deref())
                .await
                .map(|page| page.items)
                .unwrap_or_default()
        })
    }

    pub fn fetch_memories(&self) -> oneshot::Receiver<Vec<MemoryView>> {
        let client = self.client.clone();
        let Some(soul) = self.soul_id.lock().clone() else {
            let (tx, rx) = oneshot::channel();
            drop(tx.send(Vec::new()));
            return rx;
        };
        self.spawn_fetch(async move {
            client
                .list_memories(&soul, None)
                .await
                .map(|page| page.items)
                .unwrap_or_default()
        })
    }

    pub fn fetch_schedules(&self) -> oneshot::Receiver<Vec<ScheduleView>> {
        let client = self.client.clone();
        self.spawn_fetch(async move {
            client
                .list_schedules()
                .await
                .map(|page| page.items)
                .unwrap_or_default()
        })
    }

    pub fn fetch_approvals(&self) -> oneshot::Receiver<Vec<ApprovalView>> {
        let client = self.client.clone();
        self.spawn_fetch(async move {
            client
                .list_approvals()
                .await
                .map(|page| page.items)
                .unwrap_or_default()
        })
    }

    pub fn fetch_jobs(&self) -> oneshot::Receiver<Vec<JobView>> {
        let client = self.client.clone();
        let soul = self.soul_id.lock().clone();
        self.spawn_fetch(async move {
            client
                .list_jobs(soul.as_deref())
                .await
                .map(|page| page.items)
                .unwrap_or_default()
        })
    }

    pub fn fetch_souls(&self) -> oneshot::Receiver<Vec<SoulView>> {
        let client = self.client.clone();
        self.spawn_fetch(async move {
            client
                .list_souls()
                .await
                .map(|page| page.items)
                .unwrap_or_default()
        })
    }

    pub fn fetch_core_settings(&self) -> oneshot::Receiver<Result<Value, String>> {
        let client = self.client.clone();
        self.spawn_fetch(async move { client.settings().await.map_err(|err| err.to_string()) })
    }

    pub fn fetch_provider_models(
        &self,
        plugin: String,
        task: String,
        base_url: String,
        api_key: String,
    ) -> oneshot::Receiver<Result<Vec<String>, String>> {
        let client = self.client.clone();
        self.spawn_fetch(async move {
            let listed = client
                .list_provider_models(&ListProviderModelsRequest {
                    plugin,
                    task,
                    base_url,
                    api_key,
                })
                .await
                .map_err(|err| err.to_string())?;
            if listed.models.is_empty() {
                if let Some(error) = listed.error {
                    Err(error)
                } else {
                    Ok(Vec::new())
                }
            } else {
                Ok(listed.models)
            }
        })
    }

    pub fn fetch_provider_assets(
        &self,
        plugin: String,
    ) -> oneshot::Receiver<Result<Vec<ene_api::ProviderAssetView>, String>> {
        let client = self.client.clone();
        self.spawn_fetch(async move {
            let listed = client
                .list_provider_assets(&ene_api::ListProviderAssetsRequest { plugin })
                .await
                .map_err(|err| err.to_string())?;
            if let Some(error) = listed.error.filter(|_| listed.assets.is_empty()) {
                Err(error)
            } else {
                Ok(listed.assets)
            }
        })
    }

    pub fn begin_provider_asset_install(
        &self,
        plugin: String,
        asset_id: String,
        version: Option<String>,
        variant: Option<String>,
    ) -> oneshot::Receiver<Result<String, String>> {
        let client = self.client.clone();
        self.spawn_fetch(async move {
            let started = client
                .install_provider_asset(&ene_api::InstallProviderAssetRequest {
                    plugin,
                    asset_id,
                    version,
                    variant,
                })
                .await
                .map_err(|err| err.to_string())?;
            if let Some(error) = started.error.filter(|_| started.job_id.is_empty()) {
                return Err(error);
            }
            Ok(started.job_id)
        })
    }

    pub fn refresh_provider_asset_catalogs(&self) -> oneshot::Receiver<Result<(), String>> {
        let client = self.client.clone();
        self.spawn_fetch(async move {
            for plugin in ["provider.gguf", "provider.voicevox"] {
                client
                    .refresh_provider_assets_catalog(
                        &ene_api::RefreshProviderAssetsCatalogRequest {
                            plugin: plugin.to_owned(),
                        },
                    )
                    .await
                    .map_err(|err| err.to_string())?;
            }
            Ok(())
        })
    }

    pub fn poll_provider_asset_install_status(
        &self,
        plugin: String,
        job_id: String,
    ) -> oneshot::Receiver<Result<ene_api::ProviderAssetInstallStatusResponse, String>> {
        let client = self.client.clone();
        self.spawn_fetch(async move {
            client
                .provider_asset_install_status(&ene_api::ProviderAssetInstallStatusRequest {
                    plugin,
                    job_id,
                })
                .await
                .map_err(|err| err.to_string())
        })
    }

    pub fn install_provider_asset(
        &self,
        plugin: String,
        asset_id: String,
        version: Option<String>,
    ) -> oneshot::Receiver<Result<ene_api::ProviderAssetInstallStatusResponse, String>> {
        let client = self.client.clone();
        self.spawn_fetch(async move {
            let started = client
                .install_provider_asset(&ene_api::InstallProviderAssetRequest {
                    plugin: plugin.clone(),
                    asset_id,
                    version,
                    variant: None,
                })
                .await
                .map_err(|err| err.to_string())?;
            if let Some(error) = started.error.clone().filter(|_| started.job_id.is_empty()) {
                return Err(error);
            }
            for _ in 0..600 {
                let status = client
                    .provider_asset_install_status(&ene_api::ProviderAssetInstallStatusRequest {
                        plugin: plugin.clone(),
                        job_id: started.job_id.clone(),
                    })
                    .await
                    .map_err(|err| err.to_string())?;
                if matches!(
                    status.phase,
                    Some(
                        ene_api::ProviderAssetInstallPhase::Done
                            | ene_api::ProviderAssetInstallPhase::Failed,
                    )
                ) {
                    return Ok(status);
                }
                tokio::time::sleep(std::time::Duration::from_millis(250)).await;
            }
            Err("install timed out".to_owned())
        })
    }

    pub fn fetch_health(&self) -> oneshot::Receiver<Result<String, String>> {
        let client = self.client.clone();
        self.spawn_fetch(async move {
            client
                .health()
                .await
                .map(|health| format!("{} ({})", health.status, health.bind))
                .map_err(|err| err.to_string())
        })
    }

    pub fn restart_plugin(&self, id: String) -> oneshot::Receiver<Result<(), String>> {
        let client = self.client.clone();
        self.spawn_fetch(async move {
            client
                .restart_plugin(&id)
                .await
                .map(|_| ())
                .map_err(|err| err.to_string())
        })
    }

    pub fn fetch_mcp(&self) -> oneshot::Receiver<Result<McpDocument, String>> {
        let client = self.client.clone();
        self.spawn_fetch(async move { client.mcp().await.map_err(|err| err.to_string()) })
    }

    pub fn put_mcp(&self, doc: McpDocument) -> oneshot::Receiver<Result<McpDocument, String>> {
        let client = self.client.clone();
        self.spawn_fetch(async move { client.put_mcp(&doc).await.map_err(|err| err.to_string()) })
    }

    pub fn patch_memory(
        &self,
        id: String,
        content: String,
    ) -> oneshot::Receiver<Result<(), String>> {
        let client = self.client.clone();
        self.spawn_fetch(async move {
            client
                .patch_memory(
                    &id,
                    &MemoryPatch {
                        content: Some(content),
                        scope: None,
                        completed: None,
                        schedule_id: None,
                    },
                )
                .await
                .map(|_| ())
                .map_err(|err| err.to_string())
        })
    }

    pub fn delete_memory(&self, id: String) -> oneshot::Receiver<Result<(), String>> {
        let client = self.client.clone();
        self.spawn_fetch(async move {
            client
                .delete_memory(&id)
                .await
                .map_err(|err| err.to_string())
        })
    }

    pub fn archive_session(
        &self,
        id: String,
        archived: bool,
    ) -> oneshot::Receiver<Result<(), String>> {
        let client = self.client.clone();
        self.spawn_fetch(async move {
            client
                .patch_session(
                    &id,
                    &SessionPatch {
                        archived: Some(archived),
                        title: None,
                    },
                )
                .await
                .map(|_| ())
                .map_err(|err| err.to_string())
        })
    }

    pub fn export_session(&self, id: String) -> oneshot::Receiver<Result<Value, String>> {
        let client = self.client.clone();
        self.spawn_fetch(async move {
            client
                .export_session(&id)
                .await
                .map_err(|err| err.to_string())
        })
    }

    pub fn fork_session(&self, id: String) -> oneshot::Receiver<Result<SessionView, String>> {
        let client = self.client.clone();
        self.spawn_fetch(async move {
            client
                .fork_session(&id)
                .await
                .map_err(|err| err.to_string())
        })
    }

    pub fn set_schedule_enabled(
        &self,
        id: String,
        enabled: bool,
    ) -> oneshot::Receiver<Result<(), String>> {
        let client = self.client.clone();
        self.spawn_fetch(async move {
            client
                .patch_schedule(&id, enabled)
                .await
                .map(|_| ())
                .map_err(|err| err.to_string())
        })
    }

    pub fn delete_schedule(&self, id: String) -> oneshot::Receiver<Result<(), String>> {
        let client = self.client.clone();
        self.spawn_fetch(async move {
            client
                .delete_schedule(&id)
                .await
                .map_err(|err| err.to_string())
        })
    }

    pub fn create_schedule(
        &self,
        req: CreateScheduleRequest,
    ) -> oneshot::Receiver<Result<ScheduleView, String>> {
        let client = self.client.clone();
        self.spawn_fetch(async move {
            client
                .create_schedule(&req)
                .await
                .map_err(|err| err.to_string())
        })
    }

    pub fn respond_approval(
        &self,
        id: String,
        decision: String,
    ) -> oneshot::Receiver<Result<(), String>> {
        let client = self.client.clone();
        self.spawn_fetch(async move {
            client
                .respond_approval(&id, &decision)
                .await
                .map(|_| ())
                .map_err(|err| err.to_string())
        })
    }

    pub fn patch_core_settings(&self, body: Value) -> oneshot::Receiver<Result<Value, String>> {
        let client = self.client.clone();
        self.spawn_fetch(async move {
            client
                .patch_settings(&body)
                .await
                .map_err(|err| err.to_string())
        })
    }

    pub fn apply_settings_async(
        &self,
        patch: Value,
    ) -> oneshot::Receiver<Result<std::collections::BTreeSet<String>, String>> {
        let client = self.client.clone();
        self.spawn_fetch(async move {
            if patch.as_object().is_some_and(serde_json::Map::is_empty) {
                return Ok(std::collections::BTreeSet::new());
            }
            client
                .patch_settings(&patch)
                .await
                .map(|_| {
                    patch
                        .as_object()
                        .map(|object| object.keys().cloned().collect())
                        .unwrap_or_default()
                })
                .map_err(|err| err.to_string())
        })
    }

    pub fn greetings(
        &self,
        assets_dir: &std::path::Path,
        card_path: Option<&str>,
    ) -> Vec<(u32, String)> {
        let Some(card_path) = card_path else {
            return Vec::new();
        };
        let path = assets_dir.join(card_path);
        let Ok(card) = ene_card::load_character_card(&path.to_string_lossy()) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        if !card.data.first_mes.trim().is_empty() {
            out.push((0, card.data.first_mes.clone()));
        }
        for (index, greeting) in card.data.alternate_greetings.iter().enumerate() {
            if !greeting.trim().is_empty() {
                out.push((
                    u32::try_from(index + 1).unwrap_or(u32::MAX),
                    greeting.clone(),
                ));
            }
        }
        out
    }

    pub fn set_greeting_blocking(
        &self,
        _index: u32,
        text: &str,
    ) -> Result<String, CoreSessionError> {
        Ok(text.to_owned())
    }

    fn block_on<T>(
        &self,
        fut: impl std::future::Future<Output = T>,
    ) -> Result<T, CoreSessionError> {
        self.runtime
            .block_on(async { tokio::time::timeout(BLOCKING_TIMEOUT, fut).await })
            .map_err(|_| CoreSessionError::Timeout(BLOCKING_TIMEOUT.as_secs()))
    }
}

#[must_use]
pub fn kill_on_drop(lifetime: CoreLifetime) -> bool {
    matches!(lifetime, CoreLifetime::App)
}

fn surface_event_allowed(value: &Value) -> bool {
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

fn format_event_line(value: &Value) -> String {
    if let Some(text) = value.get("text").and_then(Value::as_str) {
        let kind = value.get("type").and_then(Value::as_str).unwrap_or("event");
        return format!("{kind}: {text}");
    }
    value.to_string()
}

async fn pump_events(
    client: ApiClient,
    session_id: Option<String>,
    event_tx: AppEventSender,
    processing: Arc<AtomicBool>,
    active_turn: Arc<Mutex<Option<String>>>,
    detail_lines: Arc<Mutex<Vec<String>>>,
    audio_tx: Option<crate::audio::AudioChunkSender>,
) {
    let surface = client.events("surface", session_id.as_deref()).await;
    let detail = client.events("detail", session_id.as_deref()).await;
    let Ok(mut surface) = surface else {
        drop(event_tx.send(AppEvent::RuntimeDisconnected));
        return;
    };
    let mut detail = detail.ok();
    loop {
        let detail_next = async {
            match detail.as_mut() {
                Some(socket) => socket.recv_json().await,
                None => std::future::pending().await,
            }
        };
        tokio::select! {
            surface_event = surface.recv_json() => {
                if let Ok(Some(value)) = surface_event {
                    if surface_event_allowed(&value) {
                        dispatch_surface(
                            &value,
                            &event_tx,
                            &processing,
                            &active_turn,
                            audio_tx.as_ref(),
                        );
                    }
                } else {
                    drop(event_tx.send(AppEvent::RuntimeDisconnected));
                    return;
                }
            }
            detail_event = detail_next => {
                match detail_event {
                    Ok(Some(value)) => {
                        let line = format_event_line(&value);
                        let mut lines = detail_lines.lock();
                        if lines.last() != Some(&line) {
                            lines.push(line);
                        }
                    }
                    Ok(None) | Err(_) => {
                        detail = None;
                    }
                }
            }
        }
    }
}

fn dispatch_surface(
    value: &Value,
    event_tx: &AppEventSender,
    processing: &Arc<AtomicBool>,
    active_turn: &Arc<Mutex<Option<String>>>,
    audio_tx: Option<&crate::audio::AudioChunkSender>,
) {
    let event_type = value.get("type").and_then(Value::as_str).unwrap_or("");
    match event_type {
        "text.delta" => {
            if let Some(text) = value.get("text").and_then(Value::as_str)
                && !text.is_empty()
            {
                drop(event_tx.send(AppEvent::Ai(AiStreamUpdate::TextDelta(text.to_owned()))));
            }
        }
        "session.event" => dispatch_session_event(value, event_tx, processing, active_turn),
        "tool.call" => {
            let name = value
                .get("name")
                .and_then(Value::as_str)
                .or_else(|| value.get("summary").and_then(Value::as_str))
                .unwrap_or("tool");
            let arguments = value
                .get("args")
                .map_or_else(String::new, ToString::to_string);
            drop(event_tx.send(AppEvent::Ai(AiStreamUpdate::ToolCallStart {
                name: name.to_owned(),
                arguments,
            })));
        }
        "approval.requested" => {
            let request_id = value
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            let action = value
                .get("tool")
                .and_then(Value::as_str)
                .unwrap_or("tool")
                .to_owned();
            let target = value
                .get("target")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            drop(
                event_tx.send(AppEvent::Ai(AiStreamUpdate::PermissionRequired {
                    request_id,
                    action,
                    target,
                    description: String::new(),
                })),
            );
        }
        "body.expression" => {
            if let Some(label) = value.get("label").and_then(Value::as_str) {
                drop(event_tx.send(AppEvent::ExpressionCue {
                    name: label.to_owned(),
                    weight: 0.65,
                    hold_secs: 1.5,
                    target_time: 0.0,
                }));
                drop(event_tx.send(AppEvent::PerformanceCue(label.to_owned())));
            }
        }
        "body.look_at" => {
            if let Some(target) = value.get("target").and_then(Value::as_str) {
                drop(event_tx.send(AppEvent::LookAtCue {
                    target: target.to_owned(),
                }));
            }
        }
        "job.report" => {
            if let Some(speech) = value
                .get("speech")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
            {
                drop(event_tx.send(AppEvent::Ai(AiStreamUpdate::TextDelta(speech.to_owned()))));
            }
        }
        "audio.chunk" => {
            if let Some(sender) = audio_tx
                && let Some(pcm) = value.get("pcm").and_then(Value::as_array)
            {
                let samples: Vec<f32> = pcm
                    .iter()
                    .filter_map(Value::as_f64)
                    .map(|v| v as f32)
                    .collect();
                let payload = crate::audio::AudioChunkPayload {
                    pcm: samples,
                    sample_rate: value
                        .get("sample_rate")
                        .and_then(Value::as_u64)
                        .unwrap_or(24_000) as u32,
                    is_final: value
                        .get("is_final")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                    abort: value.get("abort").and_then(Value::as_bool).unwrap_or(false),
                    cues: expression_cues_from_audio(value),
                };
                drop(sender.send(payload));
            }
        }
        _ => {}
    }
}

fn dispatch_session_event(
    value: &Value,
    event_tx: &AppEventSender,
    processing: &Arc<AtomicBool>,
    active_turn: &Arc<Mutex<Option<String>>>,
) {
    let kind = value.get("kind").and_then(Value::as_str).unwrap_or("");
    match kind {
        "turn/end" => {
            processing.store(false, Ordering::Relaxed);
            *active_turn.lock() = None;
            let outcome = value.get("outcome").and_then(Value::as_str).unwrap_or("");
            if matches!(outcome, "failed" | "interrupted" | "cancelled") {
                drop(event_tx.send(AppEvent::Ai(AiStreamUpdate::Error(outcome.to_owned()))));
            } else {
                drop(event_tx.send(AppEvent::Ai(AiStreamUpdate::Finished)));
            }
        }
        "tool/result" => {
            let name = value
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("tool")
                .to_owned();
            let result = value
                .get("result")
                .map_or_else(String::new, ToString::to_string);
            drop(event_tx.send(AppEvent::Ai(AiStreamUpdate::ToolCallResult {
                name,
                result,
            })));
        }
        "question/asked" => {
            let request_id = value
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            let title = value
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            let questions = value
                .get("questions")
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(Value::as_str)
                        .map(ToOwned::to_owned)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            if request_id.is_empty() && title.is_empty() && questions.is_empty() {
                return;
            }
            drop(
                event_tx.send(AppEvent::Ai(AiStreamUpdate::UserInputRequired {
                    request_id,
                    prompt: crate::settings::UserInputPrompt { title, questions },
                })),
            );
        }
        _ => {}
    }
}

fn expression_cues_from_audio(value: &Value) -> Vec<crate::audio::ExpressionCue> {
    value
        .get("expression")
        .and_then(Value::as_str)
        .filter(|name| !name.is_empty())
        .map(|name| vec![crate::audio::ExpressionCue::expression(name)])
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn dispatch(value: Value) -> Vec<AppEvent> {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let processing = Arc::new(AtomicBool::new(true));
        let active_turn = Arc::new(Mutex::new(Some("turn".to_owned())));
        dispatch_surface(&value, &tx, &processing, &active_turn, None);
        let mut out = Vec::new();
        while let Ok(event) = rx.try_recv() {
            out.push(event);
        }
        out
    }

    #[test]
    fn maps_tool_call_summary() {
        let events = dispatch(json!({
            "type": "tool.call",
            "name": "fs.read",
            "args": {"path": "/tmp"}
        }));
        assert!(matches!(
            events.first(),
            Some(AppEvent::Ai(AiStreamUpdate::ToolCallStart { name, .. }))
                if name == "fs.read"
        ));
    }

    #[test]
    fn maps_interrupted_turn_to_error() {
        let events = dispatch(json!({
            "type": "session.event",
            "kind": "turn/end",
            "outcome": "interrupted"
        }));
        assert!(matches!(
            events.first(),
            Some(AppEvent::Ai(AiStreamUpdate::Error(outcome))) if outcome == "interrupted"
        ));
    }

    #[test]
    fn maps_question_asked() {
        let events = dispatch(json!({
            "type": "session.event",
            "kind": "question/asked",
            "id": "q1",
            "title": "Pick one",
            "questions": ["a", "b"]
        }));
        assert!(matches!(
            events.first(),
            Some(AppEvent::Ai(AiStreamUpdate::UserInputRequired { request_id, prompt }))
                if request_id == "q1" && prompt.questions.len() == 2
        ));
    }
}
