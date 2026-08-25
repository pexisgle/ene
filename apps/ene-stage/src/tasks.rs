use ene_api::{
    JobView, MemoryCandidateView, MemoryJournalView, MemoryView, PluginView, ProviderAssetView,
    ScheduleView,
};

use crate::core::session::PreparedSessionTarget;

pub struct ActivatedCharacter {
    pub(crate) character: ene_api::CharacterView,
    pub(crate) target: Option<PreparedSessionTarget>,
}

pub enum AsyncOutcome {
    SendMessage {
        session_id: String,
        result: Result<(), String>,
    },
    BargeIn {
        session_id: String,
        result: Result<(), String>,
    },
    CancelTurn {
        session_id: String,
        result: Result<(), String>,
    },
    Approval {
        session_id: String,
        result: Result<(), String>,
    },
    SelectGreeting {
        session_id: String,
        result: Result<ene_api::HistoryResponse, String>,
    },
    Listen {
        generation: u64,
        result: Result<(), String>,
    },
    RefreshHistory {
        session_id: String,
        result: Result<ene_api::HistoryResponse, String>,
    },
    SaveLocalSettings(Result<(), String>),
    LoadCoreSettings(Result<String, String>),
    ApplyCoreSettings(Result<(), String>),
    LoadMcpCatalog(Result<ene_api::McpCatalogDocument, String>),
    ProbeMcp {
        generation: u64,
        result: Result<ene_api::McpProbeResponse, String>,
    },
    LoadTools(Result<Vec<ene_api::ToolView>, String>),
    ListMemories {
        soul_id: String,
        result: Result<Vec<MemoryView>, String>,
    },
    ListPendingMemories {
        soul_id: String,
        result: Result<Vec<MemoryCandidateView>, String>,
    },
    ListMemoryJournal {
        soul_id: String,
        result: Result<Vec<MemoryJournalView>, String>,
    },
    ResolveMemory {
        soul_id: String,
        id: String,
        result: Result<(), String>,
    },
    ResolveMemoryFailedKeepCandidate {
        soul_id: String,
        original: MemoryCandidateView,
        result: Result<(), String>,
    },
    DeleteMemory {
        soul_id: String,
        id: String,
        result: Result<(), String>,
    },
    CompleteMemory {
        soul_id: String,
        id: String,
        result: Result<(), String>,
    },
    LoadSoul(Result<ene_api::SoulView, String>),
    PatchBody(Result<ene_api::SoulView, String>),
    ImportCharacter {
        generation: u64,
        result: Result<ActivatedCharacter, String>,
    },
    ActivateCharacter {
        generation: u64,
        result: Result<ActivatedCharacter, String>,
    },
    ListCharacters(Result<Vec<ene_api::CharacterView>, String>),
    ListOccupants(Result<Vec<ene_api::OccupantView>, String>),
    ListJobs(Result<(Vec<JobView>, Vec<ScheduleView>), String>),
    CreateJob(Result<JobView, String>),
    CreateSchedule(Result<ScheduleView, String>),
    CancelJob {
        id: String,
        result: Result<(), String>,
    },
    ToggleSchedule {
        id: String,
        enabled: bool,
        result: Result<(), String>,
    },
    ListPlugins(Result<Vec<PluginView>, String>),
    RestartPlugin {
        id: String,
        result: Result<(), String>,
    },
    LoadPluginConfig {
        request_id: u64,
        id: String,
        result: Result<ene_api::PluginConfigView, String>,
    },
    ValidatePluginConfig(Result<ene_api::PluginConfigValidateView, String>),
    ApplyPluginConfig(Result<ene_api::PluginConfigValidateView, String>),
    PluginConfigOptions(Result<ene_api::PluginConfigOptionsView, String>),
    ListProviderAssets(Result<Vec<ProviderAssetView>, String>),
    InstallProviderAsset {
        asset_id: String,
        result: Result<String, String>,
    },
    ProviderAssetInstallStatus {
        asset_id: String,
        result: Result<ene_api::ProviderAssetInstallStatusResponse, String>,
    },
    SetActiveProviderAsset {
        asset_id: String,
        result: Result<(), String>,
    },
    ListProviderModels(Result<(Vec<String>, Option<String>), String>),
    LoadMcp(Result<String, String>),
    SaveMcp(Result<(), String>),
    MicClaim(Result<bool, String>),
    SpeakerClaim(Result<String, String>),
    NotifyClaim(Result<(), String>),
    Health(Result<ene_api::Health, String>),
    Usage(Result<ene_api::UsageView, String>),
    Backup(Result<(String, String), String>),
    Restore(Result<(), String>),
    DiagSpans(Result<Vec<ene_api::SpanView>, String>),
    LoadSchema(Result<String, String>),
    ListApprovals(Result<Vec<ene_api::ApprovalView>, String>),
    ReloadAvatar,
    ExportCharacter(Result<(), String>),
    ForkSession(Result<String, String>),
    NewSession(Result<ene_api::SplitSessionResponse, String>),
    CompactSession(Result<String, String>),
    ExportSession(Result<(), String>),
}
