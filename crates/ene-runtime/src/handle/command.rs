//! Commands sent to the [`super::actor::TurnActor`] from consumers (UI/CLI).
//!
//! Fire-and-forget variants are sent via internal channels. Oneshot variants
//! carry a reply channel for result confirmation.
//!
//! ## History (#271)
//!
//! Before this split, this enum also carried the read-only session/candidate
//! queries (`ListSessions`, `ExportSession`, `ImportSession`,
//! `SearchSessions`, `ArchiveSession`, `ListPendingCandidates`,
//! `ApproveCandidate`, `RejectCandidate` — 8 variants) and a
//! `SummarizeScreenImage` variant carrying a raw RGB8 buffer. The session /
//! candidate variants are gone entirely: [`crate::query::sessions::SessionQueryHandle`]
//! and [`crate::query::candidates::MemoryCandidateHandle`] talk to
//! `MemoryStore` directly, bypassing the actor mailbox (and the head-of-line
//! blocking behind in-flight `Run` turns that came with it). The vision
//! variant is now [`EneCommand::PrepareVisionSummary`] /
//! [`EneCommand::StashProactiveScreenImage`], both payload-free — see
//! [`crate::vision`] for where the buffer and the actual model call now
//! live.

use crate::error::EneRuntimeError;
use crate::streaming::{PermissionDecision, UserInputResponse};
use crate::types::{RequestId, TurnId};
use crate::vision::VisionPrepared;
use chrono::{DateTime, Utc};
use ene_config::CharacterCardV3;
use ene_mind::CompressionResult;
use ene_plugin_proto::ToolSpec;
use std::sync::Arc;
use tokio::sync::oneshot;

/// A deferred (background) tool task tracked by the actor (#196).
///
/// Created when a background-capable tool accepts a deferred call and
/// returns a `task_id`. The actor polls the owning tool until the task
/// reaches a terminal state, then emits
/// [`super::event::LifecycleEvent::ToolBackgroundCompleted`] on the
/// lifecycle bus (#272).
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
    /// List all session-wide permission grants (#177).
    ListPermissions {
        /// Reply channel for the granted scopes.
        reply: oneshot::Sender<Vec<crate::streaming::PermissionScope>>,
    },
    /// Revoke a single session-wide permission grant by id (#177).
    RevokePermission {
        /// The `PermissionScope::id` to revoke.
        id: u64,
        /// Reply channel reporting whether a scope was removed.
        reply: oneshot::Sender<bool>,
    },
    /// Revoke all session-wide permission grants (#177).
    ResetAllPermissions {
        /// Reply channel carrying the number of revoked scopes.
        reply: oneshot::Sender<usize>,
    },
    /// Undo the most recent reversible tool operation (#178).
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
    /// Compression (#368/#369) trims history into a stored scene summary but
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
    /// Cancel a deferred (background) tool task by id (#196).
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
    /// Invalidate the Tool RAG index, forcing re-embedding on next query.
    InvalidateToolIndex,
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
    /// Update host-side proactive observation snapshot (#166).
    UpdateProactiveObservation {
        /// Normalized observation from desktop (no raw screenshots).
        observation: ene_mind::ProactiveObservation,
    },
    /// Hot-update proactive policy (#103). Provider routing comes from [`AiConfig`].
    UpdateProactiveSettings {
        /// Mind proactive policy.
        mind: ene_mind::ProactiveConfig,
    },
    /// Hot-update Features-tab settings (mind / store / tools / RAG) without
    /// tearing down the local proactive GGUF.
    UpdateFeatureSettings {
        /// Boxed payload to keep [`EneCommand`] small.
        settings: Box<FeatureSettingsUpdate>,
    },
    /// Prepare a screen-image vision summary (#271).
    ///
    /// Payload-free (no RGB buffer): performs the same "runtime busy" check
    /// and lazy local-model init the legacy `SummarizeScreenImage` command
    /// did, then hands back a cloned model handle plus rendered prompts so
    /// [`crate::vision::VisionHandle`] can run the actual inference outside
    /// the actor. See [`crate::vision`] module docs.
    PrepareVisionSummary {
        /// Privacy-safe OS app label (may be empty).
        app_label: String,
        /// Reply channel.
        reply: oneshot::Sender<Result<VisionPrepared, crate::public_api::PublicApiError>>,
    },
    /// Stash the *encoded* (JPEG data URI) screen frame for the next
    /// proactive generation turn (#271). `None` clears any previous stash
    /// (mirrors the legacy inline encode-failure fallback). Fire-and-forget.
    StashProactiveScreenImage {
        /// Encoded frame, or `None` on encode failure.
        data_uri: Option<String>,
    },
    /// Internal: background plugin host reconfiguration completed (#397).
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
    /// Test-only (#268 regression coverage): mutates `pending_permissions`,
    /// `permission_scopes`, and `undo_stack` — the three fields #268 called
    /// out for post-panic consistency scrutiny — then panics, so the panic
    /// hits mid-command with in-flight shared-state mutations already
    /// applied. Exercises `run_command_isolated`'s `catch_unwind` under
    /// realistic conditions rather than a synthetic bare future. Compiled
    /// only under `cfg(test)`; not reachable from production code.
    #[cfg(test)]
    TestInjectPanicAfterMutations {
        /// Request id inserted into `pending_permissions` before the panic.
        request_id: RequestId,
        /// Reply channel stashed in `pending_permissions` before the panic;
        /// the test resolves it afterward via [`crate::EneHandle::decide_permission`]
        /// to prove the map entry survived intact.
        permission_tx: oneshot::Sender<PermissionDecision>,
    },
    /// Test-only (#397 regression coverage): occupies one `bg_command_tasks`
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

/// Payload for [`EneCommand::UpdateFeatureSettings`].
#[derive(Debug, Clone)]
pub struct FeatureSettingsUpdate {
    /// Full mind section (emotion + proactive).
    pub mind: ene_mind::MindConfig,
    /// Long-term memory store section.
    pub store: ene_store::StoreConfig,
    /// Plugin system section (formerly tool host section).
    pub plugins: ene_plugin_host::PluginConfig,
    /// Tool RAG section.
    pub rag: ene_rag::ToolRagConfig,
}
