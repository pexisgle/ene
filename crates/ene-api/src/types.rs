use serde::{Deserialize, Serialize};

/// RFC 9457-ish problem document with harness `error_class`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Problem {
    #[serde(rename = "type")]
    pub type_url: String,
    pub title: String,
    pub status: u16,
    pub error_class: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<String>,
}

impl Problem {
    #[must_use]
    pub fn new(status: u16, error_class: &str, title: &str) -> Self {
        Self {
            type_url: "about:blank".to_owned(),
            title: title.to_owned(),
            status,
            error_class: error_class.to_owned(),
            detail: None,
            turn_id: None,
        }
    }
}

/// Cursor page used by every list endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Page<T> {
    pub items: Vec<T>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

impl<T> Page<T> {
    #[must_use]
    pub fn of(items: Vec<T>) -> Self {
        Self {
            items,
            next_cursor: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Health {
    pub status: String,
    pub bind: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SoulView {
    pub id: String,
    pub character_ref: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub display_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub voice_ref: Option<String>,
    pub mood_label: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SoulPatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_ref: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionView {
    pub id: String,
    pub soul_id: String,
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub created_at: String,
    pub archived: bool,
    pub next_seq: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateSessionRequest {
    pub soul_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SessionPatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archived: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum MessageMode {
    #[default]
    Prompt,
    Steer,
    FollowUp,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageRequest {
    pub text: String,
    #[serde(default)]
    pub mode: MessageMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_modality: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SendMessageResponse {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entry_id: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageResponse {
    pub seq: u64,
    pub role: String,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryResponse {
    pub messages: Vec<MessageResponse>,
    pub depth: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactResponse {
    pub entry_id: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueuedCancel {
    pub result: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdempotentMessage {
    pub key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobView {
    pub id: String,
    pub soul_id: String,
    pub title: String,
    pub goal: String,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress_fraction: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress_note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduleView {
    pub id: String,
    pub soul_id: String,
    pub name: String,
    pub spec: String,
    pub timezone: String,
    pub action: String,
    pub enabled: bool,
    pub important: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_fire: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateScheduleRequest {
    pub soul_id: String,
    pub name: String,
    pub spec: String,
    #[serde(default = "default_tz")]
    pub timezone: String,
    #[serde(default = "default_action")]
    pub action: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action_ref: Option<String>,
    #[serde(default)]
    pub important: bool,
}

fn default_tz() -> String {
    "UTC".to_owned()
}

fn default_action() -> String {
    "remind".to_owned()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactView {
    pub id: String,
    pub soul_id: String,
    pub title: String,
    pub kind: String,
    pub path: String,
    pub delivered: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryView {
    pub id: String,
    pub soul_id: String,
    pub scope: String,
    pub kind: String,
    pub title: String,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MemoryPatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolView {
    pub name: String,
    pub description: String,
    pub layer: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ToolTestRequest {
    #[serde(default)]
    pub arguments: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginView {
    pub row_id: String,
    pub plugin: String,
    pub state: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalView {
    pub id: String,
    pub tool: String,
    pub target: String,
    pub side_effects: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CharacterView {
    pub id: String,
    pub version: String,
    pub kind: String,
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SettingsPatch {
    #[serde(flatten)]
    pub fields: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageView {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub rows: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpanView {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u128>,
    pub attrs: Vec<(String, String)>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupResponse {
    pub id: String,
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RestoreRequest {
    pub id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceKind {
    Mic,
    Speaker,
    Notify,
}

impl ResourceKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Mic => "mic",
            Self::Speaker => "speaker",
            Self::Notify => "notify",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "mic" => Some(Self::Mic),
            "speaker" => Some(Self::Speaker),
            "notify" => Some(Self::Notify),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClaimResourceRequest {
    pub client_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExclusiveSnapshot {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mic: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speaker: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notify: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OccupantView {
    pub soul_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StageView {
    pub occupants: Vec<OccupantView>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AffectView {
    pub valence: f32,
    pub arousal: f32,
    pub dominance: f32,
    pub trust: f32,
    pub affinity: f32,
    pub mood_label: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct EndSessionRequest {
    #[serde(default)]
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SplitSessionResponse {
    pub previous: SessionView,
    pub session: SessionView,
}
