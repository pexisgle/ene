//! HTTP/WS contract for `ene-core`. Transport only — no daemon types.

#![deny(unsafe_code)]

mod client;
mod error;
mod types;

pub use client::{ApiClient, EventSocket};
pub use error::ApiError;
pub use types::{
    ApprovalView, ArtifactView, BackupResponse, CharacterView, ClaimResourceRequest,
    CompactResponse, CreateScheduleRequest, CreateSessionRequest, ExclusiveSnapshot, Health,
    HistoryResponse, IdempotentMessage, JobView, MemoryPatch, MemoryView, MessageMode,
    MessageRequest, MessageResponse, Page, PluginView, Problem, QueuedCancel, ResourceKind,
    RestoreRequest, ScheduleView, SendMessageResponse, SessionPatch, SessionView, SettingsPatch,
    SoulPatch, SoulView, SpanView, ToolTestRequest, ToolView, UsageView,
};

/// `OpenAPI` 3.1 document. Served at `GET /api/v1/openapi.json`.
pub const OPENAPI_JSON: &str = include_str!("../openapi.json");

#[must_use]
pub fn openapi_json() -> &'static str {
    OPENAPI_JSON
}
