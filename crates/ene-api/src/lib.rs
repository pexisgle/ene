//! HTTP/WS contract for `ene-core`. Transport only — no daemon types.

#![deny(unsafe_code)]
#![cfg_attr(test, expect(clippy::expect_used, reason = "tests"))]

mod client;
mod error;
mod pcm;
mod types;

pub use client::{ApiClient, EventSocket, ListenStream};
pub use error::ApiError;
pub use pcm::{LISTEN_SAMPLE_RATE, PCM_S16LE, decode_pcm_s16le, encode_pcm_s16le};
pub use types::{
    AffectView, AnswerJobRequest, AnswerQuestionRequest, ApprovalView, ArtifactView,
    BackupResponse, CharacterView, ClaimResourceRequest, CompactResponse, CreateScheduleRequest,
    CreateSessionRequest, EndSessionRequest, ExclusiveSnapshot, GreetingView, Health,
    HistoryResponse, IdempotentMessage, InstallProviderAssetRequest, InstallProviderAssetResponse,
    JobView, ListProviderAssetsRequest, ListProviderAssetsResponse, ListProviderModelsRequest,
    ListProviderModelsResponse, ListenRequest, McpDocument, McpServerView, MemoryPatch, MemoryView,
    MessageMode, MessageRequest, MessageResponse, OccupantView, Page, PluginConfigErrorView,
    PluginConfigField, PluginConfigOptionView, PluginConfigOptionsView, PluginConfigValidateView,
    PluginConfigValues, PluginConfigView, PluginView, Problem, ProviderAssetInstallPhase,
    ProviderAssetInstallStatusRequest, ProviderAssetInstallStatusResponse,
    ProviderAssetVersionView, ProviderAssetView, QueuedCancel, RefreshProviderAssetsCatalogRequest,
    RefreshProviderAssetsCatalogResponse, ResourceKind, RestoreRequest, ScheduleView,
    SelectGreetingRequest, SelectGreetingResponse, SendMessageResponse, SessionPatch, SessionView,
    SetActiveProviderAssetRequest, SetActiveProviderAssetResponse, SettingsPatch, SoulPatch,
    SoulSkillsPatch, SoulView, SpanView, SplitSessionResponse, StageView, ToolTestRequest,
    ToolView, UsageView,
    MemoryCandidateDecision, MemoryCandidateView, MemoryJournalView, ResolveMemoryCandidateRequest,
    ResolveMemoryCandidateResponse,
};

/// `OpenAPI` 3.1 document. Served at `GET /api/v1/openapi.json`.
pub const OPENAPI_JSON: &str = include_str!("../openapi.json");

/// Canonical live-bus event emitted when a job needs a user answer.
pub const QUESTION_ASKED_EVENT: &str = "question.asked";

#[must_use]
pub fn openapi_json() -> &'static str {
    OPENAPI_JSON
}
