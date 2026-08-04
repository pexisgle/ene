//! Stable public API v1 surface for external clients.
//!
//! The actor facade remains [`crate::EneHandle`]. This module defines the
//! versioned JSON/serde mirror of chat events, session DTOs, the unified
//! [`PublicApiError`] category, redaction helpers, and the `API_VERSION`
//! constant. Internal modules such as `streaming` and `message_builder` are
//! not part of this contract.
//!
//! ## What is (and isn't) part of the contract
//!
//! The API v1 contract is exactly the `Public*` types defined in this module
//! (plus [`API_VERSION`] and the redaction helpers), and the subset of
//! [`crate::EneHandle`] methods whose signatures are built entirely out of
//! those `Public*` types and primitives (currently: `list_sessions`,
//! `export_session`, `import_session`, `search_sessions`,
//! `archive_session`). No type from `ene_store`, `ene_mind`, or
//! `ene_plugin_proto` appears in any `Public*` type's fields or in any of
//! those methods' signatures — see
//! `tests::public_dto_fields_are_primitive_only` below for a compile-time
//! check.
//!
//! `EneHandle` methods *not* listed above (e.g. `subscribe`,
//! `subscribe_lifecycle`, `take_audio_stream`, `diagnostics`,
//! `update_feature_settings`) are host-internal wiring between
//! `ene-runtime` and its embedders (`ene-cli`, `ene-desktop`) and are
//! explicitly out of contract, same as `streaming` / `message_builder`.
//! They may freely take or return internal crate types.
//!
//! ## Three-channel event bus
//!
//! [`PublicChatEvent`] mirrors only the chat bus ([`EneEvent`]). Lifecycle
//! notifications (`StatusChanged`, `PendingCandidateAvailable`,
//! `CandidateChanged`, `ToolBackgroundCompleted`) now ride a separate
//! lifecycle bus
//! ([`LifecycleEvent`]) and are mirrored by [`PublicLifecycleEvent`]
//! instead. The audio channel ([`crate::handle::AudioChunk`]) has no
//! `Public*` mirror: it is a heavyweight, single-consumer, in-process
//! streaming path (obtained via `EneHandle::take_audio_stream`) that has
//! never been exposed as external JSON, unlike the chat/lifecycle buses
//! which [`PublicChatEvent`] / [`PublicLifecycleEvent`] exist to mirror for
//! exactly that purpose.
//!
//! Internal `From<...>` conversions inside this module (e.g.
//! `From<ene_store::EneMemoryError> for PublicApiError`) necessarily name
//! internal types on their *input* side — that's the DTO boundary doing its
//! job, not a leak. A leak is an internal type appearing in a `Public*`
//! type's own field list or in a contract method's return/parameter type.
//!
//! ## Fixed asymmetry
//!
//! The event side ([`PublicChatEvent::from_ene_event`]) and the
//! command/response side of `EneHandle` are symmetric: session methods
//! mirror results into `Public*` DTOs rather than returning `ene_store`
//! types double-nested inside `Result<Result<T, E>, ActorDead>`. The
//! actor-control, diagnostics, and tool-handle methods likewise report a dead
//! actor as [`PublicApiError::ActorDead`] rather than a dedicated error type.

use crate::handle::{EneEvent, EneStatus, LifecycleEvent, TerminalReason};
use crate::types::TurnOrigin;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Public API major version string for ene-runtime host contracts.
///
/// ## What triggers a version bump
///
/// - Removing or renaming a `Public*` type, or a field on one.
/// - Changing the *meaning* or wire shape of an existing field (e.g.
///   `archived: bool` becoming a tri-state enum).
/// - Removing a [`PublicApiError`] variant, or changing an existing
///   variant's shape (its fields).
/// - Removing or changing the signature of one of the contract methods
///   listed in the module docs above.
///
/// ## What does NOT trigger a bump
///
/// - Adding a new variant to an *internal* error enum
///   (`ene_store::EneMemoryError` and friends) — internal variants project
///   into [`PublicApiError`] through `From` impls, and
///   `ene_store::EneMemoryError` is itself `#[non_exhaustive]`, so new
///   internal variants fall through to `PublicApiError::Internal` without
///   any code or contract change being required.
/// - Adding a new `Public*` type, a new [`PublicApiError`] variant (it is
///   `#[non_exhaustive]`), a new optional field, or a new
///   [`PublicChatEvent`] variant — clients that ignore unknown `type`/`kind`
///   tags and unknown fields are unaffected.
pub const API_VERSION: &str = "1";

/// A single presentation cue in a [`PublicChatEvent::Performance`] event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublicPerfCue {
    /// Cue name (expression / motion label).
    pub name: String,
    /// Cue kind: `expression`, `motion`, `lookat`, or `cancel`.
    pub kind: String,
    /// How the cue was chosen (e.g. `affect`, `llm`).
    pub source: String,
}

/// API v1 mirror of a stored session's metadata.
///
/// Returned by [`crate::EneHandle::list_sessions`]. Field-for-field
/// equivalent to `ene_store::SessionMeta` (never renamed or reshaped without
/// a doc-only reason). This DTO is also the canonical type the desktop UI
/// renders directly — embedders do not convert it back to the `ene_store`
/// shape.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublicSessionMeta {
    /// Row id.
    pub id: i64,
    /// Logical session key (unique across the store).
    pub session_id: String,
    /// Character card the session belongs to.
    pub card_name: String,
    /// Human-readable session title (may be empty until set).
    pub title: String,
    /// When the session was first created.
    pub created_at: DateTime<Utc>,
    /// When the session was last touched (newest-first ordering key).
    pub updated_at: DateTime<Utc>,
    /// Whether the session is archived (hidden from default listings).
    pub archived: bool,
    /// Number of conversation turns recorded for the session.
    pub turn_count: i64,
}

impl From<ene_store::SessionMeta> for PublicSessionMeta {
    fn from(m: ene_store::SessionMeta) -> Self {
        Self {
            id: m.id,
            session_id: m.session_id,
            card_name: m.card_name,
            title: m.title,
            created_at: m.created_at,
            updated_at: m.updated_at,
            archived: m.archived,
            turn_count: m.turn_count,
        }
    }
}

/// API v1 mirror of a single exported conversation message.
///
/// Returned (paired with a session id) by
/// [`crate::EneHandle::search_sessions`]. Message content has already been
/// redacted by the store before this conversion runs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublicExportedMessage {
    /// Role that produced the message (e.g. `user`, `assistant`).
    pub role: String,
    /// Message content (secrets redacted).
    pub content: String,
    /// When the message was recorded.
    pub created_at: DateTime<Utc>,
}

impl From<ene_store::ExportedMessage> for PublicExportedMessage {
    fn from(m: ene_store::ExportedMessage) -> Self {
        Self {
            role: m.role,
            content: m.content,
            created_at: m.created_at,
        }
    }
}

/// Stable error categories for API v1 request/response methods.
///
/// Internal error enums (`ene_store::EneMemoryError`) project into one of
/// these categories via the `From` impls below, so a new internal error
/// variant does not change this type — see the `API_VERSION` bump-policy doc
/// above. The actor-control methods on [`crate::EneHandle`] (permissions,
/// undo, user input, feature updates) and the diagnostics / vision / tools
/// handles report a dead actor directly as [`PublicApiError::ActorDead`] —
/// there is no separate actor-dead error type. `#[non_exhaustive]` so a
/// future category addition is itself non-breaking for match arms in client
/// code.
#[derive(Debug, Clone, Serialize, Deserialize, thiserror::Error)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum PublicApiError {
    /// The actor task backing this `EneHandle` is no longer running.
    #[error("the ene actor is no longer running")]
    ActorDead,
    /// The requested resource (e.g. a session id) does not exist.
    #[error("not found: {message}")]
    NotFound {
        /// Human-readable detail (never contains secrets).
        message: String,
    },
    /// A persistence-layer (database, backup, migration) operation failed.
    #[error("storage error: {message}")]
    Storage {
        /// Human-readable detail (never contains secrets).
        message: String,
    },
    /// The request was rejected because the input was invalid.
    #[error("invalid request: {message}")]
    Invalid {
        /// Human-readable detail (never contains secrets).
        message: String,
    },
    /// An internal failure that does not fit the other categories.
    #[error("internal error: {message}")]
    Internal {
        /// Human-readable detail (never contains secrets).
        message: String,
    },
}

impl From<ene_store::EneMemoryError> for PublicApiError {
    /// Projects every current `EneMemoryError` variant into a stable
    /// category. `EneMemoryError` is `#[non_exhaustive]`, so this
    /// match requires — and keeps — a trailing wildcard arm; new upstream
    /// variants fall through to `Internal` until this mapping is revisited,
    /// rather than failing to compile.
    fn from(err: ene_store::EneMemoryError) -> Self {
        use ene_store::EneMemoryError as E;
        let message = err.to_string();
        match &err {
            // Database / backup / migration / on-disk data problems.
            E::MemoryStoreError(_)
            | E::MemoryStoreConnectionError(_)
            | E::UnrecognizedValue { .. }
            | E::BackupError(_)
            | E::IntegrityCheckFailed(_)
            | E::SchemaTooNew { .. }
            | E::MigrationRolledBack { .. } => Self::Storage { message },
            // Caller-supplied data was rejected.
            E::InvalidEmbedding(_)
            | E::InvalidTransition { .. }
            | E::InvalidPendingCandidateEdit(_)
            | E::UnsupportedFormatVersion(_)
            | E::SerializationError(_) => Self::Invalid { message },
            // `Other` is a free-text catch-all inside `ene_store` itself; the
            // one caller-visible "not found" case (see
            // `MemoryStore::build_export`) is threaded through it because
            // `ene_store` has no dedicated not-found variant. Recognize that
            // one shape explicitly.
            E::Other(msg) if msg.contains("not found") => Self::NotFound { message },
            // Everything else maps to `Internal`: the deprecated
            // `ApiRequestError` (kept only for source compat, never
            // constructed by current `ene_store` code), the non-"not found"
            // shape of `Other`, and — since `ene_store::EneMemoryError` is
            // `#[non_exhaustive]` — any variant added upstream after this
            // match was written. This wildcard is mandatory, not dead code:
            // it's what lets a new internal variant compile cleanly against
            // this contract instead of breaking it.
            _ => Self::Internal { message },
        }
    }
}

/// Stable JSON mirror of the chat [`EneEvent`] bus.
///
/// Tagged with `type` in `snake_case`, aligned with the CLI JSONL schema and
/// extended with fields the host contract documents (`origin`, gates).
/// Prefer this type over serializing [`EneEvent`] directly when exposing
/// events outside the process. Turn-independent lifecycle notifications
/// (`StatusChanged`, `PendingCandidateAvailable`, `CandidateChanged`,
/// `ToolBackgroundCompleted`) are not part of this type — see
/// [`PublicLifecycleEvent`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PublicChatEvent {
    /// The turn has started streaming.
    TurnStarted {
        /// Turn id (UUID string).
        turn: String,
        /// Who initiated the turn.
        origin: String,
    },
    /// A chunk of generated text.
    TextDelta {
        /// Turn id.
        turn: String,
        /// Who initiated the turn.
        origin: String,
        /// Text chunk (markers stripped).
        delta: String,
    },
    /// Presentation cues for the turn.
    Performance {
        /// Turn id.
        turn: String,
        /// Who initiated the turn.
        origin: String,
        /// Cue list.
        cues: Vec<PublicPerfCue>,
        /// Aggregate cue source for the batch.
        source: String,
    },
    /// A tool call was requested.
    ToolCallStart {
        /// Turn id.
        turn: String,
        /// Who initiated the turn.
        origin: String,
        /// Tool name.
        name: String,
        /// Parsed JSON arguments (object), after redaction when converted via
        /// [`PublicChatEvent::from_ene_event`].
        arguments: serde_json::Value,
    },
    /// A tool call completed.
    ToolCallResult {
        /// Turn id.
        turn: String,
        /// Who initiated the turn.
        origin: String,
        /// Tool name.
        name: String,
        /// Tool output (may be truncated / redacted).
        result: String,
    },
    /// A destructive operation requires approval.
    PermissionRequired {
        /// Turn id.
        turn: String,
        /// Who initiated the turn.
        origin: String,
        /// Permission request id.
        request_id: String,
        /// Operation category.
        action: String,
        /// Target resource.
        target: String,
        /// Human-readable description.
        description: String,
    },
    /// An interactive tool needs user input.
    UserInputRequired {
        /// Turn id.
        turn: String,
        /// Who initiated the turn.
        origin: String,
        /// Input request id.
        request_id: String,
        /// Prompt kind label from the tool ABI.
        prompt_kind: String,
    },
    /// Rolling context compression completed for this turn.
    ContextCompressed {
        /// Turn id.
        turn: String,
        /// Who initiated the turn.
        origin: String,
        /// Compression level label.
        level: String,
    },
    /// The turn reached a terminal state (exactly one per run).
    Terminal {
        /// Turn id.
        turn: String,
        /// Who initiated the turn.
        origin: String,
        /// `done`, `failed`, or `cancelled`.
        reason: String,
        /// Error detail when `reason` is `failed`.
        #[serde(skip_serializing_if = "Option::is_none")]
        message: Option<String>,
    },
    /// A detected system-audio beat pulse (Beat Sync), turn-independent.
    BeatPulse {
        /// Estimated tempo in beats per minute.
        bpm: f32,
        /// Normalized onset strength in `[0, 1]`.
        intensity: f32,
    },
}

/// Stable JSON mirror of the lifecycle [`LifecycleEvent`] bus.
///
/// Tagged with `type` in `snake_case`, mirroring [`PublicChatEvent`]'s
/// conventions. Covers the turn-independent notifications kept off
/// [`PublicChatEvent`]: `StatusChanged`, `PendingCandidateAvailable`,
/// `CandidateChanged`, `ToolBackgroundCompleted`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PublicLifecycleEvent {
    /// Actor status changed.
    StatusChanged {
        /// `idle`, `running`, or `error`.
        status: String,
    },
    /// One or more pending memory candidates became available for review.
    PendingCandidatesAvailable {
        /// Number of pending candidates currently awaiting review.
        count: usize,
    },
    /// A pending memory candidate was approved, rejected, or edited.
    CandidateChanged {
        /// Candidate row id.
        id: i64,
        /// Status after the mutation (`pending` / `approved` / `rejected`).
        status: String,
        /// Active turn context at mutation time, when any.
        turn: Option<String>,
    },
    /// A deferred background tool task reached a terminal state.
    ToolBackgroundCompleted {
        /// Tool name.
        tool_name: String,
        /// Background task id.
        task_id: String,
        /// Terminal status string (`completed`, `failed`, `cancelled`, …).
        status: String,
    },
}

impl PublicChatEvent {
    /// Convert an internal chat event into the stable public mirror with
    /// redaction applied to tool arguments and sensitive text.
    #[must_use]
    pub fn from_ene_event(event: &EneEvent) -> Self {
        match event {
            EneEvent::TurnStarted { turn, origin } => Self::TurnStarted {
                turn: turn.to_string(),
                origin: origin_label(*origin).to_string(),
            },
            EneEvent::TextDelta {
                turn,
                origin,
                delta,
            } => Self::TextDelta {
                turn: turn.to_string(),
                origin: origin_label(*origin).to_string(),
                delta: redact_text(delta),
            },
            EneEvent::Performance {
                turn,
                origin,
                cues,
                source,
            } => Self::Performance {
                turn: turn.to_string(),
                origin: origin_label(*origin).to_string(),
                cues: cues
                    .iter()
                    .map(|c| PublicPerfCue {
                        name: c.name.clone(),
                        kind: perf_kind_label(c.kind).to_string(),
                        source: cue_source_label(*source),
                    })
                    .collect(),
                source: cue_source_label(*source),
            },
            EneEvent::ToolCallStart {
                turn,
                origin,
                name,
                arguments,
            } => Self::ToolCallStart {
                turn: turn.to_string(),
                origin: origin_label(*origin).to_string(),
                name: name.clone(),
                arguments: redact_tool_arguments_json(arguments),
            },
            EneEvent::ToolCallResult {
                turn,
                origin,
                name,
                result,
            } => Self::ToolCallResult {
                turn: turn.to_string(),
                origin: origin_label(*origin).to_string(),
                name: name.clone(),
                result: redact_text(result),
            },
            EneEvent::PermissionRequired {
                turn,
                origin,
                request_id,
                action,
                target,
                description,
            } => Self::PermissionRequired {
                turn: turn.to_string(),
                origin: origin_label(*origin).to_string(),
                request_id: request_id.to_string(),
                action: action.clone(),
                target: target.clone(),
                description: description.clone(),
            },
            EneEvent::UserInputRequired {
                turn,
                origin,
                request_id,
                prompt: _,
            } => Self::UserInputRequired {
                turn: turn.to_string(),
                origin: origin_label(*origin).to_string(),
                request_id: request_id.to_string(),
                prompt_kind: "user_input".to_string(),
            },
            EneEvent::ContextCompressed {
                turn,
                origin,
                level,
            } => Self::ContextCompressed {
                turn: turn.to_string(),
                origin: origin_label(*origin).to_string(),
                level: level.clone(),
            },
            EneEvent::Terminal {
                turn,
                origin,
                reason,
            } => {
                let (reason_label, message) = match reason {
                    TerminalReason::Done => ("done".to_string(), None),
                    TerminalReason::Failed { message } => {
                        ("failed".to_string(), Some(redact_text(message)))
                    }
                    TerminalReason::Cancelled => ("cancelled".to_string(), None),
                    TerminalReason::Declined => ("declined".to_string(), None),
                };
                Self::Terminal {
                    turn: turn.to_string(),
                    origin: origin_label(*origin).to_string(),
                    reason: reason_label,
                    message,
                }
            }
            EneEvent::BeatPulse { bpm, intensity } => Self::BeatPulse {
                bpm: *bpm,
                intensity: *intensity,
            },
        }
    }
}

impl PublicLifecycleEvent {
    /// Convert an internal lifecycle event into the stable public mirror.
    #[must_use]
    pub fn from_lifecycle_event(event: &LifecycleEvent) -> Self {
        match event {
            LifecycleEvent::StatusChanged { status } => Self::StatusChanged {
                status: match status {
                    EneStatus::Idle => "idle".to_string(),
                    EneStatus::Running => "running".to_string(),
                },
            },
            LifecycleEvent::PendingCandidateAvailable { count } => {
                Self::PendingCandidatesAvailable { count: *count }
            }
            LifecycleEvent::CandidateChanged { id, status, turn } => Self::CandidateChanged {
                id: *id,
                status: status.as_str().to_string(),
                turn: turn.as_ref().map(ToString::to_string),
            },
            LifecycleEvent::ToolBackgroundCompleted {
                tool_name,
                task_id,
                status,
            } => Self::ToolBackgroundCompleted {
                tool_name: tool_name.clone(),
                task_id: task_id.clone(),
                status: format!("{status:?}").to_ascii_lowercase(),
            },
        }
    }
}

const fn origin_label(origin: TurnOrigin) -> &'static str {
    match origin {
        TurnOrigin::User => "user",
        TurnOrigin::Proactive => "proactive",
        TurnOrigin::Scheduled => "scheduled",
    }
}

fn perf_kind_label(kind: ene_mind::PerfKind) -> &'static str {
    match kind {
        ene_mind::PerfKind::Expression => "expression",
        ene_mind::PerfKind::Motion => "motion",
        ene_mind::PerfKind::LookAt => "lookat",
        ene_mind::PerfKind::Cancel => "cancel",
    }
}

fn cue_source_label(source: ene_mind::CueSource) -> String {
    source.as_str().to_string()
}

/// Redact obvious secrets from free-form text (API keys, bearer tokens, PEM).
pub fn redact_text(input: &str) -> String {
    ene_store::redact_secrets(input)
}

/// Redact sensitive keys inside a tool-argument JSON string.
pub fn redact_tool_arguments(arguments: &str) -> String {
    ene_store::redact_arguments(arguments)
}

/// Parse tool arguments as JSON (object preferred) after redaction.
pub fn redact_tool_arguments_json(arguments: &str) -> serde_json::Value {
    let redacted = redact_tool_arguments(arguments);
    serde_json::from_str(&redacted).unwrap_or_else(|_| serde_json::json!({ "raw": redacted }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::TurnId;

    #[test]
    fn api_version_is_one() {
        assert_eq!(API_VERSION, "1");
    }

    /// API-stability regression check.
    ///
    /// Every `Public*` type's fields are constructed here using only
    /// primitives, `String`, `bool`, and `chrono::DateTime<Utc>` — never an
    /// `ene_store` / `ene_mind` / `ene_plugin_proto` type. If a future edit
    /// adds a field of one of those internal types to a `Public*` struct (or
    /// changes a field's type away from something constructible this way),
    /// this test stops *compiling*, catching the leak before it ships rather
    /// than relying on a reviewer noticing an import.
    #[test]
    fn public_dto_fields_are_primitive_only() {
        let meta = PublicSessionMeta {
            id: 1,
            session_id: "s1".to_string(),
            card_name: "card".to_string(),
            title: "title".to_string(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            archived: false,
            turn_count: 3,
        };
        assert_eq!(meta.id, 1);

        let msg = PublicExportedMessage {
            role: "user".to_string(),
            content: "hi".to_string(),
            created_at: Utc::now(),
        };
        assert_eq!(msg.role, "user");

        let cue = PublicPerfCue {
            name: "smile".to_string(),
            kind: "expression".to_string(),
            source: "affect".to_string(),
        };
        assert_eq!(cue.kind, "expression");

        let errors = [
            PublicApiError::ActorDead,
            PublicApiError::NotFound {
                message: "x".to_string(),
            },
            PublicApiError::Storage {
                message: "x".to_string(),
            },
            PublicApiError::Invalid {
                message: "x".to_string(),
            },
            PublicApiError::Internal {
                message: "x".to_string(),
            },
        ];
        assert_eq!(errors.len(), 5);
    }

    #[test]
    fn public_api_error_serializes_with_kind_tag() {
        let err = PublicApiError::NotFound {
            message: "session missing".to_string(),
        };
        let value = serde_json::to_value(&err).expect("serializable");
        assert_eq!(value["kind"], "not_found");
        assert_eq!(value["message"], "session missing");

        let value = serde_json::to_value(PublicApiError::ActorDead).expect("serializable");
        assert_eq!(value["kind"], "actor_dead");
    }

    #[test]
    fn memory_store_error_maps_to_storage() {
        let mapped: PublicApiError =
            ene_store::EneMemoryError::MemoryStoreConnectionError("disk full".to_string()).into();
        assert!(matches!(mapped, PublicApiError::Storage { .. }));
    }

    #[test]
    fn invalid_embedding_maps_to_invalid() {
        let mapped: PublicApiError =
            ene_store::EneMemoryError::InvalidEmbedding("wrong dims".to_string()).into();
        assert!(matches!(mapped, PublicApiError::Invalid { .. }));
    }

    #[test]
    fn not_found_shaped_other_maps_to_not_found() {
        let mapped: PublicApiError =
            ene_store::EneMemoryError::Other("session not found: abc".to_string()).into();
        assert!(matches!(mapped, PublicApiError::NotFound { .. }));
    }

    #[test]
    fn disabled_store_other_maps_to_internal() {
        let mapped: PublicApiError =
            ene_store::EneMemoryError::Other("Memory store is not enabled".to_string()).into();
        assert!(matches!(mapped, PublicApiError::Internal { .. }));
    }

    #[test]
    fn public_session_meta_from_store_type_round_trips_fields() {
        let now = Utc::now();
        let internal = ene_store::SessionMeta {
            id: 7,
            session_id: "sess-1".to_string(),
            card_name: "ene".to_string(),
            title: "hello".to_string(),
            created_at: now,
            updated_at: now,
            archived: true,
            turn_count: 12,
        };
        let public = PublicSessionMeta::from(internal);
        assert_eq!(public.id, 7);
        assert_eq!(public.session_id, "sess-1");
        assert_eq!(public.card_name, "ene");
        assert_eq!(public.title, "hello");
        assert_eq!(public.created_at, now);
        assert_eq!(public.updated_at, now);
        assert!(public.archived);
        assert_eq!(public.turn_count, 12);
    }

    #[test]
    fn public_chat_event_uses_snake_case_type_tag() {
        let event = PublicChatEvent::TextDelta {
            turn: "t1".into(),
            origin: "user".into(),
            delta: "hi".into(),
        };
        let value = serde_json::to_value(&event).expect("serializable");
        assert_eq!(value["type"], "text_delta");
        assert_eq!(value["origin"], "user");
    }

    #[test]
    fn beat_pulse_mirrors_verbatim() {
        let event = EneEvent::BeatPulse {
            bpm: 128.0,
            intensity: 0.4,
        };
        let public = PublicChatEvent::from_ene_event(&event);
        let PublicChatEvent::BeatPulse { bpm, intensity } = public else {
            panic!("expected BeatPulse");
        };
        assert!((bpm - 128.0).abs() < f32::EPSILON);
        assert!((intensity - 0.4).abs() < f32::EPSILON);
        let value = serde_json::to_value(&public).expect("serializable");
        assert_eq!(value["type"], "beat_pulse");
    }

    #[test]
    fn from_ene_event_redacts_tool_arguments() {
        let event = EneEvent::ToolCallStart {
            turn: TurnId::new(),
            origin: TurnOrigin::User,
            name: "fs.write".into(),
            arguments: r#"{"path":"/tmp/x","api_key":"sk-secret-value-here"}"#.into(),
        };
        let public = PublicChatEvent::from_ene_event(&event);
        let PublicChatEvent::ToolCallStart { arguments, .. } = public else {
            panic!("expected ToolCallStart");
        };
        let encoded = arguments.to_string();
        assert!(!encoded.contains("sk-secret-value-here"));
    }

    #[test]
    fn terminal_failed_carries_redacted_message() {
        let event = EneEvent::Terminal {
            turn: TurnId::new(),
            origin: TurnOrigin::User,
            reason: TerminalReason::Failed {
                message: "auth failed: Bearer abc.def-ghi".into(),
            },
        };
        let public = PublicChatEvent::from_ene_event(&event);
        let PublicChatEvent::Terminal {
            reason, message, ..
        } = public
        else {
            panic!("expected Terminal");
        };
        assert_eq!(reason, "failed");
        let msg = message.expect("message");
        assert!(!msg.contains("abc.def-ghi"));
    }

    #[test]
    fn pending_candidate_available_maps_to_dedicated_event() {
        // PendingCandidateAvailable lives on the lifecycle bus, not the
        // chat bus, so it mirrors through PublicLifecycleEvent.
        let event = LifecycleEvent::PendingCandidateAvailable { count: 3 };
        let public = PublicLifecycleEvent::from_lifecycle_event(&event);
        let PublicLifecycleEvent::PendingCandidatesAvailable { count } = public else {
            panic!("expected PendingCandidatesAvailable");
        };
        assert_eq!(count, 3);

        // The stable status contract must not be overloaded with a count string.
        let value = serde_json::to_value(&public).expect("serializable");
        assert_eq!(value["type"], "pending_candidates_available");
        assert_eq!(value["count"], 3);
    }

    #[test]
    fn candidate_changed_maps_to_dedicated_event() {
        let event = LifecycleEvent::CandidateChanged {
            id: 7,
            status: ene_store::PendingCandidateStatus::Approved,
            turn: Some(TurnId::from("turn-1")),
        };
        let public = PublicLifecycleEvent::from_lifecycle_event(&event);
        let PublicLifecycleEvent::CandidateChanged { id, status, turn } = &public else {
            panic!("expected CandidateChanged");
        };
        assert_eq!(*id, 7);
        assert_eq!(status.as_str(), "approved");
        assert_eq!(turn.as_deref(), Some("turn-1"));
        let value = serde_json::to_value(&public).expect("serializable");
        assert_eq!(value["type"], "candidate_changed");
    }

    /// `ToolBackgroundCompleted` mirrors on the dedicated
    /// `PublicLifecycleEvent` rather than `PublicChatEvent`, since it fires
    /// asynchronously after the originating turn has already completed.
    #[test]
    fn tool_background_completed_maps_to_lifecycle_event() {
        let event = LifecycleEvent::ToolBackgroundCompleted {
            tool_name: "background.sleep".to_string(),
            task_id: "task-456".to_string(),
            status: ene_plugin_proto::DeferredStatus::Completed {
                result: ene_plugin_proto::ToolResult::text("done"),
            },
        };
        let public = PublicLifecycleEvent::from_lifecycle_event(&event);
        let value = serde_json::to_value(&public).expect("serializable");
        assert_eq!(value["type"], "tool_background_completed");
        assert_eq!(value["tool_name"], "background.sleep");
        assert_eq!(value["task_id"], "task-456");
    }
}
