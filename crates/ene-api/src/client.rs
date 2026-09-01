use crate::error::ApiError;
use crate::pcm::{PCM_S16LE, encode_pcm_s16le};
use crate::types::{
    AffectView, AnswerJobRequest, AnswerQuestionRequest, ApprovalView, ArtifactView,
    BackupResponse, CharacterView, ClaimResourceRequest, CompactResponse, CreateJobRequest,
    CreateScheduleRequest, CreateSessionRequest, CreateTaskRequest, EndSessionRequest,
    ExclusiveSnapshot, GreetingView, Health, HistoryResponse, InstallProviderAssetRequest,
    InstallProviderAssetResponse, JobView, ListProviderAssetsRequest, ListProviderAssetsResponse,
    ListProviderModelsRequest, ListProviderModelsResponse, ListenRequest, McpCatalogDocument,
    McpDocument, McpProbeRequest, McpProbeResponse, MemoryCandidateView, MemoryJournalView,
    MemoryPatch, MemoryView, MessageRequest, Page, PluginConfigField, PluginConfigOptionsView,
    PluginConfigValidateView, PluginConfigValues, PluginConfigView, PluginView, Problem,
    ProviderAssetInstallStatusRequest, ProviderAssetInstallStatusResponse, QueuedCancel,
    RefreshProviderAssetsCatalogRequest, RefreshProviderAssetsCatalogResponse,
    ResolveMemoryCandidateRequest, ResolveMemoryCandidateResponse, ResourceKind, RestoreRequest,
    ScheduleView, SelectGreetingRequest, SelectGreetingResponse, SendMessageResponse, SessionPatch,
    SessionView, SetActiveProviderAssetRequest, SetActiveProviderAssetResponse, SoulPatch,
    SoulSkillsPatch, SoulView, SpanView, SplitSessionResponse, StageView, TaskView,
    ToolTestRequest, ToolView, UsageView,
};
use futures::{SinkExt, StreamExt};
use reqwest::{Client, Method, RequestBuilder};
use serde::de::DeserializeOwned;
use serde_json::Value;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use url::Url;

/// Typed HTTP + WebSocket client for `/api/v1`.
#[derive(Clone)]
pub struct ApiClient {
    http: Client,
    base: String,
    token: String,
    client_id: String,
}

impl ApiClient {
    #[must_use]
    pub fn new(
        base: impl Into<String>,
        token: impl Into<String>,
        client_id: impl Into<String>,
    ) -> Self {
        Self {
            http: Client::new(),
            base: base.into().trim_end_matches('/').to_owned(),
            token: token.into(),
            client_id: client_id.into(),
        }
    }

    #[must_use]
    pub fn base(&self) -> &str {
        &self.base
    }

    #[must_use]
    pub fn client_id(&self) -> &str {
        &self.client_id
    }

    #[must_use]
    pub fn token(&self) -> &str {
        &self.token
    }

    fn request(&self, method: Method, path: &str) -> RequestBuilder {
        self.http
            .request(method, format!("{}{path}", self.base))
            .bearer_auth(&self.token)
            .header("X-Client-Id", &self.client_id)
    }

    async fn send_json<T: DeserializeOwned>(&self, builder: RequestBuilder) -> Result<T, ApiError> {
        let response = builder
            .send()
            .await
            .map_err(|err| ApiError::Transport(err.to_string()))?;
        let status = response.status();
        let bytes = response
            .bytes()
            .await
            .map_err(|err| ApiError::Transport(err.to_string()))?;
        if status.is_success() {
            serde_json::from_slice(&bytes).map_err(|err| ApiError::Codec(err.to_string()))
        } else {
            let problem = serde_json::from_slice::<Problem>(&bytes).unwrap_or_else(|_| {
                Problem::new(
                    status.as_u16(),
                    "fault",
                    std::str::from_utf8(&bytes).unwrap_or("request failed"),
                )
            });
            Err(ApiError::from_problem(status.as_u16(), problem))
        }
    }

    async fn send_empty(&self, builder: RequestBuilder) -> Result<(), ApiError> {
        let _: Value = self.send_json(builder).await?;
        Ok(())
    }

    pub async fn health(&self) -> Result<Health, ApiError> {
        self.send_json(self.request(Method::GET, "/api/v1/health"))
            .await
    }

    pub async fn list_souls(&self) -> Result<Page<SoulView>, ApiError> {
        self.send_json(self.request(Method::GET, "/api/v1/souls"))
            .await
    }

    pub async fn get_soul(&self, id: &str) -> Result<SoulView, ApiError> {
        self.send_json(self.request(Method::GET, &format!("/api/v1/souls/{id}")))
            .await
    }

    pub async fn list_greetings(&self, soul_id: &str) -> Result<Page<GreetingView>, ApiError> {
        self.send_json(self.request(Method::GET, &format!("/api/v1/souls/{soul_id}/greetings")))
            .await
    }

    pub async fn patch_soul_body(&self, id: &str, patch: &SoulPatch) -> Result<SoulView, ApiError> {
        self.send_json(
            self.request(Method::PATCH, &format!("/api/v1/souls/{id}/body"))
                .json(patch),
        )
        .await
    }

    pub async fn patch_soul_skills(
        &self,
        id: &str,
        patch: &SoulSkillsPatch,
    ) -> Result<SoulView, ApiError> {
        self.send_json(
            self.request(Method::PATCH, &format!("/api/v1/souls/{id}/skills"))
                .json(patch),
        )
        .await
    }

    pub async fn list_sessions(
        &self,
        soul_id: Option<&str>,
    ) -> Result<Page<SessionView>, ApiError> {
        self.search_sessions(soul_id, None).await
    }

    pub async fn search_sessions(
        &self,
        soul_id: Option<&str>,
        q: Option<&str>,
    ) -> Result<Page<SessionView>, ApiError> {
        let query = session_search_query(soul_id, q);
        let path = if query.is_empty() {
            "/api/v1/sessions".to_owned()
        } else {
            format!("/api/v1/sessions?{query}")
        };
        self.send_json(self.request(Method::GET, &path)).await
    }

    pub async fn create_session(
        &self,
        req: &CreateSessionRequest,
    ) -> Result<SessionView, ApiError> {
        self.send_json(self.request(Method::POST, "/api/v1/sessions").json(req))
            .await
    }

    pub async fn get_session(&self, id: &str) -> Result<SessionView, ApiError> {
        self.send_json(self.request(Method::GET, &format!("/api/v1/sessions/{id}")))
            .await
    }

    pub async fn patch_session(
        &self,
        id: &str,
        patch: &SessionPatch,
    ) -> Result<SessionView, ApiError> {
        self.send_json(
            self.request(Method::PATCH, &format!("/api/v1/sessions/{id}"))
                .json(patch),
        )
        .await
    }

    pub async fn fork_session(&self, id: &str) -> Result<SessionView, ApiError> {
        self.send_json(self.request(Method::POST, &format!("/api/v1/sessions/{id}/fork")))
            .await
    }

    pub async fn split_session(&self, id: &str) -> Result<SplitSessionResponse, ApiError> {
        self.send_json(self.request(Method::POST, &format!("/api/v1/sessions/{id}/split")))
            .await
    }

    pub async fn end_session(
        &self,
        id: &str,
        req: &EndSessionRequest,
    ) -> Result<SessionView, ApiError> {
        self.send_json(
            self.request(Method::POST, &format!("/api/v1/sessions/{id}/end"))
                .json(req),
        )
        .await
    }

    pub async fn barge_in(&self, id: &str) -> Result<Value, ApiError> {
        self.send_json(self.request(Method::POST, &format!("/api/v1/sessions/{id}/barge-in")))
            .await
    }

    pub async fn listen(
        &self,
        id: &str,
        req: &ListenRequest,
    ) -> Result<SendMessageResponse, ApiError> {
        self.send_json(
            self.request(Method::POST, &format!("/api/v1/sessions/{id}/listen"))
                .json(req),
        )
        .await
    }

    /// Open a bulk mic PCM socket. Binary frames are [`PCM_S16LE`] at `sample_rate`.
    pub async fn listen_stream(
        &self,
        session_id: &str,
        sample_rate: u32,
    ) -> Result<ListenStream, ApiError> {
        let rate = sample_rate.max(1);
        let path = format!("/api/v1/sessions/{session_id}/listen/stream");
        let stream = self
            .connect_ws(
                &path,
                &[
                    ("sample_rate", rate.to_string()),
                    ("encoding", PCM_S16LE.to_owned()),
                ],
            )
            .await?;
        Ok(ListenStream { stream })
    }

    pub async fn stage(&self) -> Result<StageView, ApiError> {
        self.send_json(self.request(Method::GET, "/api/v1/stage"))
            .await
    }

    pub async fn soul_affect(&self, id: &str) -> Result<AffectView, ApiError> {
        self.send_json(self.request(Method::GET, &format!("/api/v1/souls/{id}/affect")))
            .await
    }

    pub async fn export_session(&self, id: &str) -> Result<Value, ApiError> {
        self.send_json(self.request(Method::POST, &format!("/api/v1/sessions/{id}/export")))
            .await
    }

    pub async fn send_message(
        &self,
        session_id: &str,
        req: &MessageRequest,
        idempotency_key: Option<&str>,
    ) -> Result<SendMessageResponse, ApiError> {
        let mut builder = self
            .request(
                Method::POST,
                &format!("/api/v1/sessions/{session_id}/messages"),
            )
            .json(req);
        if let Some(key) = idempotency_key {
            builder = builder.header("Idempotency-Key", key);
        }
        self.send_json(builder).await
    }

    pub async fn history(
        &self,
        session_id: &str,
        depth: &str,
    ) -> Result<HistoryResponse, ApiError> {
        self.send_json(self.request(
            Method::GET,
            &format!("/api/v1/sessions/{session_id}/history?depth={depth}"),
        ))
        .await
    }

    pub async fn select_greeting(
        &self,
        session_id: &str,
        index: u32,
    ) -> Result<SelectGreetingResponse, ApiError> {
        self.send_json(
            self.request(
                Method::POST,
                &format!("/api/v1/sessions/{session_id}/greeting"),
            )
            .json(&SelectGreetingRequest { index }),
        )
        .await
    }

    pub async fn cancel_queued(
        &self,
        session_id: &str,
        entry_id: u64,
    ) -> Result<QueuedCancel, ApiError> {
        self.send_json(self.request(
            Method::DELETE,
            &format!("/api/v1/sessions/{session_id}/queued/{entry_id}"),
        ))
        .await
    }

    pub async fn compact(&self, session_id: &str) -> Result<CompactResponse, ApiError> {
        self.send_json(self.request(
            Method::POST,
            &format!("/api/v1/sessions/{session_id}/compact"),
        ))
        .await
    }

    pub async fn cancel_turn(&self, turn_id: &str) -> Result<Value, ApiError> {
        self.send_json(self.request(Method::POST, &format!("/api/v1/turns/{turn_id}/cancel")))
            .await
    }

    pub async fn list_jobs(&self, soul_id: Option<&str>) -> Result<Page<JobView>, ApiError> {
        let path = match soul_id {
            Some(soul) => format!("/api/v1/jobs?soul_id={soul}"),
            None => "/api/v1/jobs".to_owned(),
        };
        self.send_json(self.request(Method::GET, &path)).await
    }

    pub async fn create_job(&self, req: &CreateJobRequest) -> Result<JobView, ApiError> {
        self.send_json(self.request(Method::POST, "/api/v1/jobs").json(req))
            .await
    }

    pub async fn get_job(&self, id: &str) -> Result<JobView, ApiError> {
        self.send_json(self.request(Method::GET, &format!("/api/v1/jobs/{id}")))
            .await
    }

    pub async fn cancel_job(&self, id: &str) -> Result<JobView, ApiError> {
        self.send_json(self.request(Method::POST, &format!("/api/v1/jobs/{id}/cancel")))
            .await
    }

    pub async fn answer_job(&self, id: &str, req: &AnswerJobRequest) -> Result<JobView, ApiError> {
        self.send_json(
            self.request(Method::POST, &format!("/api/v1/jobs/{id}/answer"))
                .json(req),
        )
        .await
    }

    pub async fn answer_question(
        &self,
        job_id: &str,
        question_id: &str,
        req: &AnswerQuestionRequest,
    ) -> Result<JobView, ApiError> {
        self.send_json(
            self.request(
                Method::POST,
                &format!("/api/v1/jobs/{job_id}/questions/{question_id}/answer"),
            )
            .json(req),
        )
        .await
    }

    pub async fn list_tasks(&self, soul_id: Option<&str>) -> Result<Page<TaskView>, ApiError> {
        let path = match soul_id {
            Some(soul) => format!("/api/v1/tasks?soul_id={soul}"),
            None => "/api/v1/tasks".to_owned(),
        };
        self.send_json(self.request(Method::GET, &path)).await
    }

    pub async fn create_task(&self, req: &CreateTaskRequest) -> Result<TaskView, ApiError> {
        self.send_json(self.request(Method::POST, "/api/v1/tasks").json(req))
            .await
    }

    pub async fn get_task(&self, id: &str) -> Result<TaskView, ApiError> {
        self.send_json(self.request(Method::GET, &format!("/api/v1/tasks/{id}")))
            .await
    }

    pub async fn cancel_task(&self, id: &str) -> Result<TaskView, ApiError> {
        self.send_json(self.request(Method::POST, &format!("/api/v1/tasks/{id}/cancel")))
            .await
    }

    pub async fn verify_task(&self, id: &str) -> Result<TaskView, ApiError> {
        self.send_json(self.request(Method::POST, &format!("/api/v1/tasks/{id}/verify")))
            .await
    }

    pub async fn approve_task_scope(&self, id: &str) -> Result<TaskView, ApiError> {
        self.send_json(self.request(Method::POST, &format!("/api/v1/tasks/{id}/scope-approval")))
            .await
    }

    pub async fn list_schedules(&self) -> Result<Page<ScheduleView>, ApiError> {
        self.send_json(self.request(Method::GET, "/api/v1/schedules"))
            .await
    }

    pub async fn create_schedule(
        &self,
        req: &CreateScheduleRequest,
    ) -> Result<ScheduleView, ApiError> {
        self.send_json(self.request(Method::POST, "/api/v1/schedules").json(req))
            .await
    }

    pub async fn patch_schedule(&self, id: &str, enabled: bool) -> Result<ScheduleView, ApiError> {
        self.send_json(
            self.request(Method::PATCH, &format!("/api/v1/schedules/{id}"))
                .json(&serde_json::json!({ "enabled": enabled })),
        )
        .await
    }

    pub async fn delete_schedule(&self, id: &str) -> Result<(), ApiError> {
        self.send_empty(self.request(Method::DELETE, &format!("/api/v1/schedules/{id}")))
            .await
    }

    pub async fn list_artifacts(
        &self,
        soul_id: Option<&str>,
    ) -> Result<Page<ArtifactView>, ApiError> {
        let path = match soul_id {
            Some(soul) => format!("/api/v1/artifacts?soul_id={soul}"),
            None => "/api/v1/artifacts".to_owned(),
        };
        self.send_json(self.request(Method::GET, &path)).await
    }

    pub async fn artifact_content(&self, id: &str) -> Result<Value, ApiError> {
        self.send_json(self.request(Method::GET, &format!("/api/v1/artifacts/{id}/content")))
            .await
    }

    pub async fn list_memories(
        &self,
        soul_id: &str,
        scope: Option<&str>,
    ) -> Result<Page<MemoryView>, ApiError> {
        let path = match scope {
            Some(scope) => format!("/api/v1/souls/{soul_id}/memories?scope={scope}"),
            None => format!("/api/v1/souls/{soul_id}/memories"),
        };
        self.send_json(self.request(Method::GET, &path)).await
    }

    pub async fn patch_memory(
        &self,
        id: &str,
        patch: &MemoryPatch,
    ) -> Result<MemoryView, ApiError> {
        self.send_json(
            self.request(Method::PATCH, &format!("/api/v1/memories/{id}"))
                .json(patch),
        )
        .await
    }

    pub async fn delete_memory(&self, id: &str) -> Result<(), ApiError> {
        self.send_empty(self.request(Method::DELETE, &format!("/api/v1/memories/{id}")))
            .await
    }

    pub async fn list_pending_memories(
        &self,
        soul_id: &str,
    ) -> Result<Page<MemoryCandidateView>, ApiError> {
        self.send_json(self.request(
            Method::GET,
            &format!("/api/v1/memories/pending?soul_id={soul_id}"),
        ))
        .await
    }

    pub async fn resolve_memory_candidate(
        &self,
        id: &str,
        request: &ResolveMemoryCandidateRequest,
    ) -> Result<ResolveMemoryCandidateResponse, ApiError> {
        self.send_json(
            self.request(
                Method::POST,
                &format!("/api/v1/memories/candidates/{id}/resolve"),
            )
            .json(request),
        )
        .await
    }

    pub async fn list_memory_journal(
        &self,
        soul_id: &str,
    ) -> Result<Page<MemoryJournalView>, ApiError> {
        self.send_json(self.request(
            Method::GET,
            &format!("/api/v1/memories/journal?soul_id={soul_id}"),
        ))
        .await
    }

    pub async fn list_tools(&self) -> Result<Page<ToolView>, ApiError> {
        self.send_json(self.request(Method::GET, "/api/v1/tools"))
            .await
    }

    pub async fn test_tool(&self, name: &str, req: &ToolTestRequest) -> Result<Value, ApiError> {
        self.send_json(
            self.request(Method::POST, &format!("/api/v1/tools/{name}/test"))
                .json(req),
        )
        .await
    }

    pub async fn list_plugins(&self) -> Result<Page<PluginView>, ApiError> {
        self.send_json(self.request(Method::GET, "/api/v1/plugins"))
            .await
    }

    pub async fn list_provider_models(
        &self,
        req: &ListProviderModelsRequest,
    ) -> Result<ListProviderModelsResponse, ApiError> {
        self.send_json(
            self.request(Method::POST, "/api/v1/providers/models")
                .json(req),
        )
        .await
    }

    pub async fn list_provider_assets(
        &self,
        req: &ListProviderAssetsRequest,
    ) -> Result<ListProviderAssetsResponse, ApiError> {
        self.send_json(
            self.request(Method::POST, "/api/v1/providers/assets/list")
                .json(req),
        )
        .await
    }

    pub async fn install_provider_asset(
        &self,
        req: &InstallProviderAssetRequest,
    ) -> Result<InstallProviderAssetResponse, ApiError> {
        self.send_json(
            self.request(Method::POST, "/api/v1/providers/assets/install")
                .json(req),
        )
        .await
    }

    pub async fn provider_asset_install_status(
        &self,
        req: &ProviderAssetInstallStatusRequest,
    ) -> Result<ProviderAssetInstallStatusResponse, ApiError> {
        self.send_json(
            self.request(Method::POST, "/api/v1/providers/assets/install/status")
                .json(req),
        )
        .await
    }

    pub async fn refresh_provider_assets_catalog(
        &self,
        req: &RefreshProviderAssetsCatalogRequest,
    ) -> Result<RefreshProviderAssetsCatalogResponse, ApiError> {
        self.send_json(
            self.request(Method::POST, "/api/v1/providers/assets/refresh_catalog")
                .json(req),
        )
        .await
    }

    pub async fn set_active_provider_asset(
        &self,
        req: &SetActiveProviderAssetRequest,
    ) -> Result<SetActiveProviderAssetResponse, ApiError> {
        self.send_json(
            self.request(Method::POST, "/api/v1/providers/assets/set_active")
                .json(req),
        )
        .await
    }

    pub async fn restart_plugin(&self, id: &str) -> Result<PluginView, ApiError> {
        self.send_json(self.request(Method::POST, &format!("/api/v1/plugins/{id}/restart")))
            .await
    }

    pub async fn plugin_config(&self, id: &str) -> Result<PluginConfigView, ApiError> {
        self.send_json(self.request(Method::GET, &format!("/api/v1/plugins/{id}/config")))
            .await
    }

    pub async fn validate_plugin_config(
        &self,
        id: &str,
        body: &PluginConfigValues,
    ) -> Result<PluginConfigValidateView, ApiError> {
        self.send_json(
            self.request(
                Method::POST,
                &format!("/api/v1/plugins/{id}/config/validate"),
            )
            .json(body),
        )
        .await
    }

    pub async fn plugin_config_options(
        &self,
        id: &str,
        body: &PluginConfigField,
    ) -> Result<PluginConfigOptionsView, ApiError> {
        self.send_json(
            self.request(
                Method::POST,
                &format!("/api/v1/plugins/{id}/config/options"),
            )
            .json(body),
        )
        .await
    }

    pub async fn apply_plugin_config(
        &self,
        id: &str,
        body: &PluginConfigValues,
    ) -> Result<PluginConfigValidateView, ApiError> {
        self.send_json(
            self.request(Method::PUT, &format!("/api/v1/plugins/{id}/config"))
                .json(body),
        )
        .await
    }

    pub async fn mcp(&self) -> Result<McpDocument, ApiError> {
        self.send_json(self.request(Method::GET, "/api/v1/mcp"))
            .await
    }

    pub async fn put_mcp(&self, body: &McpDocument) -> Result<McpDocument, ApiError> {
        self.send_json(self.request(Method::PUT, "/api/v1/mcp").json(body))
            .await
    }

    pub async fn mcp_catalog(&self) -> Result<McpCatalogDocument, ApiError> {
        self.send_json(self.request(Method::GET, "/api/v1/mcp/catalog"))
            .await
    }

    pub async fn probe_mcp(&self, body: &McpProbeRequest) -> Result<McpProbeResponse, ApiError> {
        self.send_json(self.request(Method::POST, "/api/v1/mcp/probe").json(body))
            .await
    }

    pub async fn list_approvals(&self) -> Result<Page<ApprovalView>, ApiError> {
        self.send_json(self.request(Method::GET, "/api/v1/approvals"))
            .await
    }

    pub async fn respond_approval(&self, id: &str, decision: &str) -> Result<Value, ApiError> {
        self.send_json(
            self.request(Method::POST, &format!("/api/v1/approvals/{id}/respond"))
                .json(&serde_json::json!({ "decision": decision })),
        )
        .await
    }

    pub async fn list_characters(&self) -> Result<Page<CharacterView>, ApiError> {
        self.send_json(self.request(Method::GET, "/api/v1/characters"))
            .await
    }

    pub async fn import_character(&self, path: &str) -> Result<CharacterView, ApiError> {
        self.send_json(
            self.request(Method::POST, "/api/v1/characters/import")
                .json(&serde_json::json!({ "path": path })),
        )
        .await
    }

    pub async fn import_character_archive_b64(
        &self,
        archive_b64: &str,
    ) -> Result<CharacterView, ApiError> {
        self.send_json(
            self.request(Method::POST, "/api/v1/characters/import")
                .json(&serde_json::json!({ "archive_b64": archive_b64 })),
        )
        .await
    }

    pub async fn export_character(&self, id: &str) -> Result<Value, ApiError> {
        self.send_json(self.request(Method::GET, &format!("/api/v1/characters/{id}/export")))
            .await
    }

    pub async fn activate_character(&self, id: &str) -> Result<CharacterView, ApiError> {
        self.send_json(self.request(Method::POST, &format!("/api/v1/characters/{id}/activate")))
            .await
    }

    pub async fn settings(&self) -> Result<Value, ApiError> {
        self.send_json(self.request(Method::GET, "/api/v1/settings"))
            .await
    }

    pub async fn patch_settings(&self, body: &Value) -> Result<Value, ApiError> {
        self.send_json(self.request(Method::PATCH, "/api/v1/settings").json(body))
            .await
    }

    pub async fn settings_schema(&self) -> Result<Value, ApiError> {
        self.send_json(self.request(Method::GET, "/api/v1/settings/schema"))
            .await
    }

    pub async fn audit(&self) -> Result<Value, ApiError> {
        self.send_json(self.request(Method::GET, "/api/v1/audit"))
            .await
    }

    pub async fn usage(&self, session_id: Option<&str>) -> Result<UsageView, ApiError> {
        let path = match session_id {
            Some(id) => format!("/api/v1/usage?session_id={id}"),
            None => "/api/v1/usage".to_owned(),
        };
        self.send_json(self.request(Method::GET, &path)).await
    }

    pub async fn diag_spans(&self) -> Result<Page<SpanView>, ApiError> {
        self.send_json(self.request(Method::GET, "/api/v1/diag/spans"))
            .await
    }

    pub async fn backup(&self) -> Result<BackupResponse, ApiError> {
        self.send_json(self.request(Method::POST, "/api/v1/backup"))
            .await
    }

    pub async fn restore(&self, req: &RestoreRequest) -> Result<Value, ApiError> {
        self.send_json(self.request(Method::POST, "/api/v1/restore").json(req))
            .await
    }

    pub async fn exclusive(&self) -> Result<ExclusiveSnapshot, ApiError> {
        self.send_json(self.request(Method::GET, "/api/v1/exclusive"))
            .await
    }

    pub async fn claim_resource(
        &self,
        kind: ResourceKind,
        req: &ClaimResourceRequest,
    ) -> Result<ExclusiveSnapshot, ApiError> {
        self.send_json(
            self.request(
                Method::POST,
                &format!("/api/v1/exclusive/{}", kind.as_str()),
            )
            .json(req),
        )
        .await
    }

    pub async fn release_resource(
        &self,
        kind: ResourceKind,
    ) -> Result<ExclusiveSnapshot, ApiError> {
        self.send_json(self.request(
            Method::DELETE,
            &format!(
                "/api/v1/exclusive/{}?client_id={}",
                kind.as_str(),
                self.client_id
            ),
        ))
        .await
    }

    pub async fn openapi(&self) -> Result<Value, ApiError> {
        self.send_json(self.request(Method::GET, "/api/v1/openapi.json"))
            .await
    }

    /// Open a depth-filtered event socket. `depth` is `surface` or `detail`.
    pub async fn events(
        &self,
        depth: &str,
        session_id: Option<&str>,
    ) -> Result<EventSocket, ApiError> {
        let mut extra = vec![
            ("depth", depth.to_owned()),
            ("client_id", self.client_id.clone()),
        ];
        if let Some(session) = session_id {
            extra.push(("session_id", session.to_owned()));
        }
        let stream = self.connect_ws("/api/v1/events", &extra).await?;
        Ok(EventSocket { stream })
    }

    async fn connect_ws(
        &self,
        path: &str,
        extra: &[(&str, String)],
    ) -> Result<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
        ApiError,
    > {
        let ws_base = if let Some(rest) = self.base.strip_prefix("https://") {
            format!("wss://{rest}")
        } else if let Some(rest) = self.base.strip_prefix("http://") {
            format!("ws://{rest}")
        } else {
            return Err(ApiError::Websocket("base URL must be http(s)".to_owned()));
        };
        let mut url = Url::parse(&format!("{ws_base}{path}"))
            .map_err(|err| ApiError::Websocket(err.to_string()))?;
        {
            let mut query = url.query_pairs_mut();
            query.append_pair("access_token", &self.token);
            for (key, value) in extra {
                query.append_pair(key, value);
            }
        }
        let mut request = url
            .as_str()
            .into_client_request()
            .map_err(|err| ApiError::Websocket(err.to_string()))?;
        use reqwest::header::HeaderValue;
        request.headers_mut().insert(
            "Sec-WebSocket-Protocol",
            HeaderValue::from_str(&format!("bearer.{}", self.token)).map_err(
                |err: reqwest::header::InvalidHeaderValue| ApiError::Websocket(err.to_string()),
            )?,
        );
        let (stream, _) = connect_async(request)
            .await
            .map_err(|err| ApiError::Websocket(err.to_string()))?;
        Ok(stream)
    }
}

/// Live event stream (server already filtered by `depth`).
pub struct EventSocket {
    stream: tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
}

impl EventSocket {
    pub async fn recv_json(&mut self) -> Result<Option<Value>, ApiError> {
        loop {
            match self.stream.next().await {
                Some(Err(err)) => return Err(ApiError::Websocket(err.to_string())),
                Some(Ok(Message::Text(text))) => {
                    let value = serde_json::from_str(text.as_ref())
                        .map_err(|err| ApiError::Codec(err.to_string()))?;
                    return Ok(Some(value));
                }
                Some(Ok(Message::Ping(payload))) => {
                    self.stream
                        .send(Message::Pong(payload))
                        .await
                        .map_err(|err| ApiError::Websocket(err.to_string()))?;
                }
                None | Some(Ok(Message::Close(_))) => return Ok(None),
                Some(Ok(_)) => {}
            }
        }
    }
}

/// Client → core bulk mic PCM (`pcm_s16le` binary frames).
pub struct ListenStream {
    stream: tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
}

impl ListenStream {
    /// Send one mono `f32` frame packed as [`PCM_S16LE`].
    pub async fn send_pcm(&mut self, pcm: &[f32]) -> Result<(), ApiError> {
        self.stream
            .send(Message::Binary(encode_pcm_s16le(pcm).into()))
            .await
            .map_err(|err| ApiError::Websocket(err.to_string()))
    }

    /// Drive ping/pong and observe a server close. `None` means the socket ended.
    pub async fn recv(&mut self) -> Result<Option<()>, ApiError> {
        loop {
            match self.stream.next().await {
                Some(Err(err)) => return Err(ApiError::Websocket(err.to_string())),
                Some(Ok(Message::Ping(payload))) => {
                    self.stream
                        .send(Message::Pong(payload))
                        .await
                        .map_err(|err| ApiError::Websocket(err.to_string()))?;
                }
                None | Some(Ok(Message::Close(_))) => return Ok(None),
                Some(Ok(_)) => return Ok(Some(())),
            }
        }
    }
}

fn session_search_query(soul_id: Option<&str>, q: Option<&str>) -> String {
    let mut pairs: Vec<(&str, &str)> = Vec::new();
    if let Some(soul) = soul_id {
        pairs.push(("soul_id", soul));
    }
    if let Some(q) = q {
        pairs.push(("q", q));
    }
    pairs
        .into_iter()
        .map(|(key, value)| {
            let encoded: String = url::form_urlencoded::byte_serialize(value.as_bytes()).collect();
            format!("{key}={encoded}")
        })
        .collect::<Vec<_>>()
        .join("&")
}

#[cfg(test)]
mod tests {
    use super::{ApiClient, session_search_query};

    #[test]
    fn new_strips_trailing_slash_and_stores_identity() {
        let client = ApiClient::new("http://127.0.0.1:9/", "tok", "stage");
        assert_eq!(client.base(), "http://127.0.0.1:9");
        assert_eq!(client.token(), "tok");
        assert_eq!(client.client_id(), "stage");
    }

    #[test]
    fn session_search_query_encodes_optional_pairs() {
        assert_eq!(session_search_query(None, None), "");
        assert_eq!(session_search_query(Some("s"), None), "soul_id=s");
        assert_eq!(session_search_query(None, Some("a b")), "q=a+b");
        assert_eq!(
            session_search_query(Some("s"), Some("a b")),
            "soul_id=s&q=a+b"
        );
    }
}
