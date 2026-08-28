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
    /// Installed package id@version when the soul is bound to a package.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package_id: Option<String>,
    /// Absolute path to a VRM (or other avatar file) inside the package.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub avatar_path: Option<String>,
    /// Enabled skill names. Empty means every installed skill is eligible.
    #[serde(default)]
    pub skill_refs: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SoulPatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_ref: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SoulSkillsPatch {
    #[serde(default)]
    pub skill_refs: Vec<String>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delegation_id: Option<String>,
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
pub struct ListenRequest {
    pub pcm: Vec<f32>,
    pub sample_rate: u32,
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
pub struct GreetingView {
    pub index: u32,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SelectGreetingRequest {
    pub index: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SelectGreetingResponse {
    pub committed: bool,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateJobRequest {
    pub soul_id: String,
    pub goal: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AnswerJobRequest {
    #[serde(default)]
    pub text: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub answers: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnswerQuestionRequest {
    pub text: String,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schedule_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryCandidateView {
    pub id: String,
    pub soul_id: String,
    pub scope: String,
    pub kind: String,
    pub title: String,
    pub content: String,
    pub confidence: f32,
    pub sensitive: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryCandidateDecision {
    Accept,
    Reject,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolveMemoryCandidateRequest {
    pub decision: MemoryCandidateDecision,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolveMemoryCandidateResponse {
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory: Option<MemoryView>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryJournalView {
    pub seq: u64,
    pub ts: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_id: Option<String>,
    pub soul_id: String,
    pub action: String,
    pub payload: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MemoryPatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schedule_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolView {
    pub name: String,
    pub description: String,
    pub layer: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub side_effects: Vec<String>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wait_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginConfigView {
    pub row_id: String,
    pub plugin: String,
    pub has_config: bool,
    pub schema: serde_json::Value,
    pub values: serde_json::Value,
    #[serde(default)]
    pub secret_keys: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PluginConfigValues {
    #[serde(default)]
    pub values: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PluginConfigField {
    #[serde(default)]
    pub field: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginConfigErrorView {
    pub path: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginConfigValidateView {
    pub ok: bool,
    #[serde(default)]
    pub errors: Vec<PluginConfigErrorView>,
    #[serde(default)]
    pub restart_required: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginConfigOptionsView {
    #[serde(default)]
    pub options: Vec<PluginConfigOptionView>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default)]
    pub fallback: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginConfigOptionView {
    pub id: String,
    pub label: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct McpServerView {
    pub id: String,
    pub transport: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct McpDocument {
    #[serde(default)]
    pub servers: Vec<McpServerView>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum McpCatalogAuthView {
    None,
    ApiKeyHeader,
    Oauth2Remote,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpCatalogEntryView {
    pub id: String,
    pub label: String,
    pub description: String,
    pub transport: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    pub auth: McpCatalogAuthView,
    pub side_effects: Vec<String>,
    pub source_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpCatalogDocument {
    /// Provenance of the table; v1 is the compiled-in static allowlist.
    pub source: String,
    /// What happens when a future dynamic catalog refresh fails.
    pub fallback: String,
    #[serde(default)]
    pub entries: Vec<McpCatalogEntryView>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct McpProbeRequest {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub transport: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Optional credential for the one-shot probe. The daemon stores it in
    /// the vault and never echoes it back.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_token: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct McpProbeResponse {
    #[serde(default)]
    pub ok: bool,
    #[serde(default)]
    pub error: Option<String>,
    /// Whether a credential is already stored for the candidate id.
    #[serde(default)]
    pub stored_auth: bool,
    /// Tools seen during the temporary pre-enable connection.
    #[serde(default)]
    pub tools: Vec<ToolView>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalView {
    pub id: String,
    pub tool: String,
    pub target: String,
    pub side_effects: Vec<String>,
    /// Model call this approval gates; empty on records created before the
    /// field existed.
    #[serde(default)]
    pub call_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CharacterView {
    pub id: String,
    pub version: String,
    pub kind: String,
    pub path: String,
    /// Soul created or reused when this package was activated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub soul_id: Option<String>,
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
    /// Human-readable name resolved from the installed companion package.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub display_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub avatar_path: Option<String>,
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

/// `POST /api/v1/providers/models` body. `api_key` is never a query param.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ListProviderModelsRequest {
    pub plugin: String,
    pub task: String,
    #[serde(default)]
    pub base_url: String,
    #[serde(default)]
    pub api_key: String,
}

/// Vendor model ids from a provider plugin (`list_models` IPC).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ListProviderModelsResponse {
    #[serde(default)]
    pub models: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// `POST /api/v1/providers/assets/list` body.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ListProviderAssetsRequest {
    pub plugin: String,
}

/// One installable version row on the assets list.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProviderAssetVersionView {
    pub version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<u64>,
    #[serde(default)]
    pub recommended: bool,
    #[serde(default)]
    pub installed: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub variant_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub label: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub backend: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub release_tag: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RefreshProviderAssetsCatalogRequest {
    pub plugin: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RefreshProviderAssetsCatalogResponse {
    #[serde(default)]
    pub refreshed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// One catalog row from `provider.assets`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProviderAssetView {
    pub id: String,
    pub kind: String,
    pub label: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub recommended: bool,
    #[serde(default)]
    pub installed: bool,
    #[serde(default)]
    pub active: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_path: Option<String>,
    #[serde(default)]
    pub versions: Vec<ProviderAssetVersionView>,
    #[serde(default)]
    pub seams: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ListProviderAssetsResponse {
    #[serde(default)]
    pub assets: Vec<ProviderAssetView>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct InstallProviderAssetRequest {
    pub plugin: String,
    pub asset_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variant: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct InstallProviderAssetResponse {
    pub job_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProviderAssetInstallStatusRequest {
    pub plugin: String,
    pub job_id: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProviderAssetInstallPhase {
    Pending,
    Downloading,
    Verifying,
    Done,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProviderAssetInstallStatusResponse {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phase: Option<ProviderAssetInstallPhase>,
    #[serde(default)]
    pub received: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SetActiveProviderAssetRequest {
    pub plugin: String,
    pub asset_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SetActiveProviderAssetResponse {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}
