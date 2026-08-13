//! Commands sent to the [`super::actor::TurnActor`] from consumers (UI/CLI).
//!
//! Fire-and-forget variants are sent via internal channels. Oneshot variants
//! carry a reply channel for result confirmation.
//!
//! ## Scope
//!
//! Read-only session/candidate queries (`ListSessions`, `ExportSession`,
//! `ImportSession`, `SearchSessions`, `ArchiveSession`) and the screen-image
//! vision buffer are handled outside this actor mailbox: see
//! [`crate::query::sessions::SessionQueryHandle`] and [`crate::vision`].
//! Pending-candidate **reads** (`list` / `inspect` / `history`) are likewise
//! mailbox-free on [`crate::query::candidates::MemoryCandidateHandle`], but
//! candidate **mutations** (`ResolveCandidate`, `EditCandidate`) cross the
//! mailbox so they serialize with turn execution, carry the active `TurnId`,
//! and emit `LifecycleEvent::CandidateChanged` audit events. The vision path
//! only crosses the mailbox as the payload-free
//! [`EneCommand::PrepareVisionSummary`] /
//! [`EneCommand::StashProactiveScreenImage`] pair.

use crate::error::EneRuntimeError;
use crate::streaming::{PermissionDecision, UserInputResponse};
use crate::types::{RequestId, TurnId};
use crate::vision::VisionPrepared;
use chrono::{DateTime, Utc};
use ene_card::CharacterCardV3;
use ene_connector::{
    AccountCredentials, AuthenticatedAccount, ConnectorError, ConnectorId, HealthStatus,
};
use ene_mind::CompressionResult;
use ene_plugin_proto::ToolSpec;
use std::sync::Arc;
use tokio::sync::oneshot;

/// A deferred (background) tool task tracked by the actor.
///
/// Created when a background-capable tool accepts a deferred call and
/// returns a `task_id`. The actor polls the owning tool until the task
/// reaches a terminal state, then emits
/// [`super::event::LifecycleEvent::ToolBackgroundCompleted`] on the
/// lifecycle bus.
#[derive(Debug, Clone)]
pub struct DeferredToolTask {
    /// The tool name that owns the background task.
    pub tool_name: String,
    /// The `task_id` returned by the deferred call acceptance.
    pub task_id: String,
    /// JSON-encoded arguments the task was started with.
    pub arguments: String,
    /// When the task was accepted for background execution.
    pub started_at: DateTime<Utc>,
}

/// Commands sent to the actor from consumers (UI/CLI).
///
/// Fire-and-forget variants are sent via internal channels. Oneshot variants
/// carry a reply channel for result confirmation.
pub enum EneCommand {
    /// Start an AI completion for the given user prompt.
    Run {
        /// The raw user input to send to the LLM.
        input: String,
        /// Turn id allocated by the handle.
        turn: TurnId,
    },
    /// Cancel a specific in-flight turn.
    Cancel {
        /// Turn to cancel.
        turn: TurnId,
    },
    /// Shut down the actor and clean up background tasks.
    Shutdown,
    /// Submit a permission decision for a pending destructive operation.
    PermissionDecision {
        /// The `request_id` from a prior `PermissionRequired` event.
        request_id: RequestId,
        /// The user's decision.
        decision: PermissionDecision,
    },
    /// List all session-wide permission grants.
    ListPermissions {
        /// Reply channel for the granted scopes.
        reply: oneshot::Sender<Vec<crate::streaming::PermissionScope>>,
    },
    /// Revoke a single session-wide permission grant by id.
    RevokePermission {
        /// The `PermissionScope::id` to revoke.
        id: u64,
        /// Reply channel reporting whether a scope was removed.
        reply: oneshot::Sender<bool>,
    },
    /// Revoke all session-wide permission grants.
    ResetAllPermissions {
        /// Reply channel carrying the number of revoked scopes.
        reply: oneshot::Sender<usize>,
    },
    /// Run a connector connectivity check (read-only, audited).
    ConnectorCheck {
        /// Connector to probe.
        id: ConnectorId,
        /// Reply channel carrying the check result.
        reply: oneshot::Sender<Result<HealthStatus, ConnectorError>>,
    },
    /// Connect a connector (permission-gated, audited).
    ConnectorConnect {
        /// Connector to authenticate with.
        id: ConnectorId,
        /// Credential handled inside the protected store boundary.
        credential: AccountCredentials,
        /// Reply channel carrying the authenticated accounts.
        reply: oneshot::Sender<Result<Vec<AuthenticatedAccount>, ConnectorError>>,
    },
    /// Disconnect one account of a connector (permission-gated, audited).
    ConnectorDisconnect {
        /// Connector owning the account.
        id: ConnectorId,
        /// Account id to disconnect.
        account: String,
        /// Reply channel carrying the outcome.
        reply: oneshot::Sender<Result<(), ConnectorError>>,
    },
    /// Record a per-action connector grant (audited).
    ConnectorGrant {
        /// Connector owning the action.
        id: ConnectorId,
        /// Action being granted.
        action: String,
        /// Target prefix the grant covers.
        target_pattern: String,
        /// Reply channel carrying the outcome.
        reply: oneshot::Sender<Result<(), ConnectorError>>,
    },
    /// Remove a per-action connector grant (audited).
    ConnectorRevoke {
        /// Connector owning the action.
        id: ConnectorId,
        /// Action being revoked.
        action: String,
        /// Target prefix being revoked.
        target_pattern: String,
        /// Reply channel reporting whether a grant was removed.
        reply: oneshot::Sender<Result<bool, ConnectorError>>,
    },
    /// Undo the most recent reversible tool operation.
    Undo {
        /// Reply channel carrying the undo report.
        reply: oneshot::Sender<crate::undo::UndoReport>,
    },
    /// Submit a user-input response for a pending interactive tool.
    UserInputResponse {
        /// The `request_id` from a prior `UserInputRequired` event.
        request_id: RequestId,
        /// The user's response (selected option, free-text, or cancel).
        response: UserInputResponse,
    },
    /// Request a read-only snapshot of the current actor state (for CLI queries).
    GetSnapshot {
        /// Reply channel for the snapshot.
        reply: oneshot::Sender<super::event::EneStateSnapshot>,
    },
    /// Request the full conversation history only.
    ///
    /// The lightweight state reads (card name, session id, turn count,
    /// config, card) are mailbox-free on [`crate::EneHandle`]; history is a
    /// large payload that stays mailbox-based, and this command lets a
    /// consumer fetch just it without paying for a full snapshot.
    GetHistory {
        /// Reply channel carrying the history entries.
        reply: oneshot::Sender<Vec<ene_mind::HistoryEntry>>,
    },
    /// Manually trigger a compression-only pass over the current conversation.
    ///
    /// Compression trims history into a stored scene summary but
    /// does **not** start a new session: the session id is unchanged and the
    /// result is a [`CompressionResult`], not a [`ene_mind::SplitResult`].
    CompressContext {
        /// Result channel carrying the compression result or an error.
        reply: oneshot::Sender<Result<CompressionResult, EneRuntimeError>>,
    },
    /// List all tools in the active tool registry.
    ListTools {
        /// Reply channel for the tools.
        reply: oneshot::Sender<Vec<ToolSpec>>,
    },
    /// Search tools in the active tool registry using RAG if available.
    SearchTools {
        /// The query to search for.
        query: String,
        /// Reply channel for the matching tools. `Err(EneRuntimeError::Busy)`
        /// when the actor's `search_tasks` `JoinSet` is at capacity (Stage 8).
        reply: oneshot::Sender<Result<Vec<ToolSpec>, EneRuntimeError>>,
    },
    /// Call a tool by name with JSON-encoded arguments.
    CallTool {
        /// The tool name.
        name: String,
        /// JSON-encoded arguments.
        arguments: String,
        /// Active turn for call-context propagation. `None` for
        /// diagnostic / background tool calls outside a turn.
        turn: Option<TurnId>,
        /// Reply channel.
        reply: oneshot::Sender<Result<String, EneRuntimeError>>,
    },
    /// Fire a schedule occurrence (sent by the scheduler timer task).
    ScheduleFire {
        /// The schedule whose occurrence is due.
        schedule_id: i64,
        /// The occurrence time the timer read from the store; the claim only
        /// proceeds when the store's `next_run_at` still equals this value.
        scheduled_at: DateTime<Utc>,
    },
    /// A scheduled run's confirmation prompt timed out.
    ScheduleConfirmationTimeout {
        /// The request id from the emitted `PermissionRequired` event.
        request_id: RequestId,
        /// Owning schedule.
        schedule_id: i64,
        /// The run waiting for approval.
        run_id: i64,
    },
    /// A scheduled tool action finished (sent by the spawned tool task).
    ScheduleToolFinished {
        /// Owning schedule.
        schedule_id: i64,
        /// The run that executed the action.
        run_id: i64,
        /// The turn the action ran under.
        turn: TurnId,
        /// The tool name, for the `ToolCallResult` event.
        tool_name: String,
        /// A permission prompt was denied (or timed out / cancelled), so the
        /// run is terminal `denied` rather than a retryable failure.
        denied: bool,
        /// The tool result text, or the failure message.
        result: Result<String, String>,
    },
    /// Create a schedule.
    AddSchedule {
        /// The new schedule definition.
        new: ene_core::NewSchedule,
        /// Reply with the persisted schedule.
        reply: oneshot::Sender<Result<ene_core::Schedule, EneRuntimeError>>,
    },
    /// List all schedules.
    ListSchedules {
        /// Reply with the schedules ordered by name.
        reply: oneshot::Sender<Result<Vec<ene_core::Schedule>, EneRuntimeError>>,
    },
    /// List recent run history for one schedule.
    ListScheduleRuns {
        /// Owning schedule.
        schedule_id: i64,
        /// Maximum number of rows, newest first.
        limit: u64,
        /// Reply with the run rows.
        reply: oneshot::Sender<Result<Vec<ene_core::ScheduleRun>, EneRuntimeError>>,
    },
    /// Delete a schedule and its history.
    DeleteSchedule {
        /// Schedule to delete.
        schedule_id: i64,
        /// Reply with whether a row was removed.
        reply: oneshot::Sender<Result<bool, EneRuntimeError>>,
    },
    /// Pause or resume a schedule.
    SetScheduleEnabled {
        /// Schedule to toggle.
        schedule_id: i64,
        /// Whether it may fire.
        enabled: bool,
        /// Reply with whether a row was updated.
        reply: oneshot::Sender<Result<bool, EneRuntimeError>>,
    },
    /// Cancel a deferred (background) tool task by id.
    ///
    /// Routes to the owning tool and asks it to abort the background task.
    /// The reply reports whether a running task was actually cancelled.
    CancelDeferredTool {
        /// The tool name that owns the background task.
        tool_name: String,
        /// The `task_id` returned by the deferred call acceptance.
        task_id: String,
        /// Reply channel carrying whether a running task was cancelled.
        reply: oneshot::Sender<bool>,
    },
    /// Resolve a pending memory candidate (approve or reject).
    ///
    /// Executed against the memory store inside the actor so the decision is
    /// serialized with turn execution and emitted as a
    /// [`super::event::LifecycleEvent::CandidateChanged`] audit event.
    ResolveCandidate {
        /// Candidate row id.
        id: i64,
        /// Target workflow status (`approved` or `rejected`).
        status: ene_store::PendingCandidateStatus,
        /// Active turn context for the audit event (`None` outside a turn).
        turn: Option<TurnId>,
        /// Reply channel.
        reply: oneshot::Sender<Result<(), crate::public_api::PublicApiError>>,
    },
    /// Edit the user-editable fields of a still-pending memory candidate.
    ///
    /// Executed against the memory store inside the actor so the edit is
    /// serialized with turn execution and emitted as a
    /// [`super::event::LifecycleEvent::CandidateChanged`] audit event.
    EditCandidate {
        /// Candidate row id.
        id: i64,
        /// New field values (validated before any write).
        edit: ene_store::PendingCandidateEdit,
        /// Active turn context for the audit event (`None` outside a turn).
        turn: Option<TurnId>,
        /// Reply channel.
        reply: oneshot::Sender<Result<(), crate::public_api::PublicApiError>>,
    },
    /// Edit a persisted typed memory in place (title / content / kind /
    /// confidence).
    ///
    /// Executed against the memory store inside the actor so the edit is
    /// serialized with turn execution and emitted as a
    /// [`super::event::LifecycleEvent::MemoryLedgerChanged`] audit event. The
    /// actor also refreshes the row's embeddings in the background so vector
    /// recall does not serve stale text.
    EditMemory {
        /// Typed-memory row id.
        id: i64,
        /// New field values (validated before any write).
        edit: ene_store::MemoryEdit,
        /// Active turn context for the audit event (`None` outside a turn).
        turn: Option<TurnId>,
        /// Reply channel.
        reply: oneshot::Sender<Result<(), crate::public_api::PublicApiError>>,
    },
    /// Set the salience (importance / Preference weight) of a typed memory.
    ///
    /// Executed against the memory store inside the actor so the adjustment
    /// is serialized with turn execution and emitted as a
    /// [`super::event::LifecycleEvent::MemoryLedgerChanged`] audit event.
    SetMemorySalience {
        /// Typed-memory row id.
        id: i64,
        /// New salience value (clamped into `0.0..=1.0` by the store).
        salience: f32,
        /// Active turn context for the audit event (`None` outside a turn).
        turn: Option<TurnId>,
        /// Reply channel.
        reply: oneshot::Sender<Result<(), crate::public_api::PublicApiError>>,
    },
    /// Invalidate the Tool RAG index, forcing re-embedding on next query.
    InvalidateToolIndex,
    /// Start a background workspace document index sync.
    WorkspaceStartSync {
        /// Reply channel. `Err(EneRuntimeError::Busy)` when a sync is
        /// already running.
        reply: oneshot::Sender<Result<(), EneRuntimeError>>,
    },
    /// Cancel the in-flight workspace sync, if any.
    WorkspaceCancelSync,
    /// Current workspace index + sync status.
    WorkspaceStatus {
        /// Reply channel carrying the status view.
        reply: oneshot::Sender<crate::workspace::WorkspaceStatusView>,
    },
    /// Hybrid search over the permitted workspace folders.
    WorkspaceSearch {
        /// Query text.
        query: String,
        /// Maximum number of hits.
        limit: usize,
        /// Reply channel carrying the hits or an error.
        reply: oneshot::Sender<Result<Vec<ene_core::WorkspaceChunkHit>, EneRuntimeError>>,
    },
    /// Persist the `CCv3` character-memory content hash after startup warmup.
    SetCcv3MemoryHash {
        /// Combined lorebook + style content hash.
        hash: u64,
        /// Confirmation channel.
        reply: oneshot::Sender<()>,
    },
    /// Replace the loaded character card.
    SetCharacter {
        /// New character card.
        card: Box<CharacterCardV3>,
        /// Confirmation channel.
        reply: oneshot::Sender<Result<(), EneRuntimeError>>,
    },
    /// Open the session with the greeting at `index` (`0` = `first_mes`,
    /// `i+1` = `alternate_greetings[i]`).
    SetGreeting {
        /// Greeting index into the current character card.
        index: u32,
        /// Reply channel carrying the applied greeting text.
        reply: oneshot::Sender<Result<String, crate::public_api::PublicApiError>>,
    },
    /// Update host-side proactive observation snapshot.
    UpdateProactiveObservation {
        /// Normalized observation from desktop (no raw screenshots).
        observation: ene_mind::ProactiveObservation,
    },
    /// Relay a detected system-audio beat pulse to chat-bus subscribers.
    ///
    /// The producer (desktop beat sync) normalizes `bpm` and `intensity`;
    /// the actor broadcasts [`super::event::EneEvent::BeatPulse`] verbatim.
    BeatPulse {
        /// Estimated tempo in beats per minute.
        bpm: f32,
        /// Normalized onset strength in `[0, 1]`.
        intensity: f32,
    },
    /// Apply a unified settings draft.
    ///
    /// Replaces the previous `UpdateProactiveSettings` /
    /// `UpdateFeatureSettings` split: the actor diffs the proposed config
    /// against its live copy, writes the changed sections, reacts per section
    /// (proactive abort, TTS provider rebuild, plugin-host reconfigure on
    /// enable-set or MCP-server changes), and reports the actual impact
    /// through the reply channel. The revision is echoed back so the UI can
    /// detect a stale apply.
    ApplySettings {
        /// Boxed payload to keep [`EneCommand`] small.
        request: Box<crate::settings::SettingsApplyRequest>,
        /// Reply channel carrying the apply outcome.
        reply: oneshot::Sender<Result<crate::settings::SettingsApplyResult, EneRuntimeError>>,
    },
    /// Fetch settings snapshots for every configured plugin (plugin center).
    GetPluginSnapshots {
        /// Reply channel carrying the snapshots.
        reply: oneshot::Sender<Vec<ene_plugin_host::PluginSettingsSnapshot>>,
    },
    /// Fetch the host-side artifact snapshot (Engines page).
    GetArtifactSnapshot {
        /// Reply channel carrying the snapshot.
        reply: oneshot::Sender<Vec<ene_plugin_host::ArtifactSnapshot>>,
    },
    /// Install or update an artifact from the signed catalog (Engines page).
    InstallArtifact {
        /// Artifact id from the catalog.
        artifact_id: String,
        /// Optional version pin; `None` installs the catalog default.
        version: Option<String>,
        /// Reply channel carrying the installed artifact view.
        reply: oneshot::Sender<Result<ene_plugin_host::InstalledArtifactView, String>>,
    },
    /// Roll an artifact back one generation (Engines page).
    RollbackArtifact {
        /// Artifact id from the catalog.
        artifact_id: String,
        /// Reply channel carrying the rolled-back artifact view.
        reply: oneshot::Sender<Result<ene_plugin_host::InstalledArtifactView, String>>,
    },
    /// Force-refresh the signed catalog (Engines page).
    RefreshCatalog {
        /// Reply channel carrying the new catalog version.
        reply: oneshot::Sender<Result<u64, String>>,
    },
    /// Fetch dynamic config options for one plugin config path
    /// (plugin-center wiring of `ListConfigOptions`).
    ListPluginConfigOptions {
        /// Plugin name.
        plugin: String,
        /// Dotted path inside the plugin config blob.
        path: String,
        /// Reply channel carrying the options.
        reply: oneshot::Sender<Result<Vec<ene_plugin_proto::ConfigOption>, EneRuntimeError>>,
    },
    /// Validate a plugin config value through the plugin's own validator
    /// (plugin-center wiring of `ValidateConfig`).
    ValidatePluginConfig {
        /// Plugin name.
        plugin: String,
        /// Proposed config value.
        value: serde_json::Value,
        /// Reply channel carrying field-level errors (empty = valid).
        reply: oneshot::Sender<Result<Vec<ene_plugin_proto::ConfigFieldError>, EneRuntimeError>>,
    },
    /// List plugin binaries discovered on disk but not configured.
    GetDiscoveredPlugins {
        /// Reply channel carrying the names.
        reply: oneshot::Sender<Vec<String>>,
    },
    /// List MCP server liveness statuses (plugin center).
    ListMcpStatuses {
        /// Reply channel carrying the statuses.
        reply: oneshot::Sender<Vec<ene_plugin_host::McpServerStatus>>,
    },
    /// Answer a schedule-run confirmation prompt (approve or deny) from the
    /// Schedules management page, by run identity rather than request id.
    ResolveScheduleConfirmation {
        /// Owning schedule.
        schedule_id: i64,
        /// The run waiting for confirmation.
        run_id: i64,
        /// `true` approves the run, `false` denies it.
        approve: bool,
        /// Reply channel carrying whether a matching pending confirmation
        /// was found and resolved.
        reply: oneshot::Sender<Result<bool, EneRuntimeError>>,
    },
    /// Update an existing schedule's editable fields.
    UpdateSchedule {
        /// Schedule row id.
        id: i64,
        /// New field values (validated exactly like [`EneCommand::AddSchedule`]).
        new: ene_core::NewSchedule,
        /// Reply channel carrying the updated schedule.
        reply: oneshot::Sender<Result<ene_core::Schedule, EneRuntimeError>>,
    },
    /// Prepare a screen-image vision summary.
    ///
    /// Payload-free (no RGB buffer): performs the runtime "busy" check and
    /// lazy local-model init, then hands back a cloned model handle plus
    /// rendered prompts so [`crate::vision::VisionHandle`] can run the
    /// actual inference outside the actor. See [`crate::vision`] module docs.
    PrepareVisionSummary {
        /// Privacy-safe OS app label (may be empty).
        app_label: String,
        /// Non-image context hints for the summary prompt (layout, window
        /// heuristic, OCR text). Small text only — never pixel data.
        hints: crate::vision::ScreenSummaryHints,
        /// Reply channel.
        reply: oneshot::Sender<Result<VisionPrepared, crate::public_api::PublicApiError>>,
    },
    /// Stash the *encoded* (JPEG data URI) screen frame for the next
    /// proactive generation turn. `None` clears any previous stash (used
    /// when encoding the frame failed). Fire-and-forget.
    StashProactiveScreenImage {
        /// Encoded frame, or `None` on encode failure.
        data_uri: Option<String>,
    },
    /// Internal: background plugin host reconfiguration completed.
    ///
    /// Sent from a `bg_command_tasks` task back to the actor after the
    /// expensive `PluginHostManager::start` I/O finishes. The shared
    /// `plugin_host` and `health_bridge_handle` mutexes have already been
    /// updated by the background task; this command carries the registry
    /// state that only the actor may update on its own fields.
    PluginHostReconfigured {
        /// Rebuilt composite tool registry from the new host.
        registry: Arc<dyn ene_plugin_host::ToolRegistry>,
        /// Per-plugin tool registries for future re-merges.
        plugin_tool_registries: Vec<Arc<dyn ene_plugin_host::ToolRegistry>>,
    },
    /// Internal: the live TTS provider may hold a dead plugin connection
    /// and must be rebuilt from the audio registry.
    ///
    /// Sent by the plugin health bridges when a permanently-disabled plugin
    /// owned the selected TTS kind, and by the host-reconfiguration task
    /// when the tool registry failed to rebuild (the reconfiguration path
    /// normally rebuilds TTS via [`Self::PluginHostReconfigured`]).
    RebuildTtsProvider,
    /// Internal: a plugin broker request needs interactive approval.
    ///
    /// The actor registers `reply` in `pending_permissions` (keyed by
    /// `request_id`) and broadcasts
    /// [`EneEvent::BrokerApprovalRequired`](crate::handle::EneEvent::BrokerApprovalRequired)
    /// so the UI can show a confirmation. The existing
    /// [`Self::PermissionDecision`] handling resolves `reply`.
    BrokerApprovalRequested {
        /// Unique request id (shown to the UI).
        request_id: RequestId,
        /// Plugin requesting the capability.
        plugin: String,
        /// Approval category (e.g. `FsRead`, `DynamicHttps`).
        category: String,
        /// Audit-safe target description.
        target: String,
        /// Human-readable description for the dialog.
        description: String,
        /// Decision channel resolved by the user's answer.
        reply: tokio::sync::oneshot::Sender<PermissionDecision>,
    },
    /// Internal: a plugin was permanently disabled and its provider
    /// factories must be evicted from the host registry.
    ///
    /// Sent by the plugin health bridges. The actor locks the shared plugin
    /// host, evicts the plugin's LLM/embedding/TTS/STT/VAD factories, and
    /// rebuilds the live TTS provider when one of the evicted kinds was
    /// selected.
    PluginProviderDisabled {
        /// Name of the disabled plugin whose factories to evict.
        plugin: String,
        /// Factory handles the emitting host generation contributed,
        /// captured by the health bridge. Eviction is identity-gated on
        /// these, so a stale event cannot evict a replacement host's
        /// factories.
        factories: ene_plugin_host::PluginFactoryHandles,
    },
    /// Probe every chat failover candidate through the provider host and
    /// return the resulting health reports.
    ///
    /// Runs in a background task so a slow probe cannot stall the actor
    /// loop. Used by the CLI `/doctor` fallback check; the shared health
    /// monitor is updated as a side effect.
    ProbeChatCandidates {
        /// Reply channel carrying one report per probed candidate.
        reply: oneshot::Sender<Vec<ene_ai::ProviderHealthReport>>,
    },
    /// Build a chat provider for the configured chat task through the
    /// provider host.
    ///
    /// Used by CLI commands that need a provider outside a turn (e.g. the
    /// memory-write retry drain), where no `StreamContext` exists.
    CreateChatProvider {
        /// Reply channel carrying the provider or a string error.
        reply: oneshot::Sender<Result<Arc<dyn ene_ai::LlmProvider>, String>>,
    },
    /// Build an STT provider for the given kind through the provider host.
    ///
    /// Used by the desktop microphone capture path, which runs outside the
    /// actor and cannot reach the plugin host directly.
    CreateSttProvider {
        /// Provider kind (the `ai.stt.provider` value).
        kind: String,
        /// Reply channel carrying the provider or a typed audio error.
        reply: oneshot::Sender<Result<Box<dyn ene_ai::SttProvider>, ene_ai::AudioProviderError>>,
    },
    /// Build a VAD engine for the given kind through the provider host.
    ///
    /// Used by the desktop microphone capture path, which runs outside the
    /// actor and cannot reach the plugin host directly.
    CreateVadEngine {
        /// Engine kind (the `ai.vad.provider` value, `"silero"` when unset).
        kind: String,
        /// Reply channel carrying the engine or a typed audio error.
        reply: oneshot::Sender<Result<Box<dyn ene_ai::VadEngine>, ene_ai::AudioProviderError>>,
    },
    /// Test-only: mutates `pending_permissions`, `permission_scopes`, and
    /// `undo_stack` — the three shared-state fields a panicking command can
    /// mutate — then panics, so the panic hits mid-command with in-flight
    /// shared-state mutations already applied. Exercises
    /// `run_command_isolated`'s `catch_unwind` under realistic conditions
    /// rather than a synthetic bare future. Compiled only under `cfg(test)`;
    /// not reachable from production code.
    #[cfg(test)]
    TestInjectPanicAfterMutations {
        /// Request id inserted into `pending_permissions` before the panic.
        request_id: RequestId,
        /// Reply channel stashed in `pending_permissions` before the panic;
        /// the test resolves it afterward via [`crate::EneHandle::decide_permission`]
        /// to prove the map entry survived intact.
        permission_tx: oneshot::Sender<PermissionDecision>,
    },
    /// Test-only: occupies one `bg_command_tasks`
    /// slot with a long-sleeping task, then replies on `reply`. Used to
    /// simulate a heavy background command (GGUF load / plugin host restart)
    /// being in flight so a follow-up command can be asserted to still be
    /// processed promptly — i.e. the actor loop is not head-of-line blocked.
    /// Compiled only under `cfg(test)`; not reachable from production code.
    #[cfg(test)]
    TestSpawnSlowBgTask {
        /// Reply channel; fired once the slow background task has been
        /// admitted and spawned, so the test knows the slot is occupied.
        reply: oneshot::Sender<()>,
    },
}
