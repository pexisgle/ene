//! Dialogue-lane kernel: one session, one running turn, model-visible = logged.

#![cfg_attr(
    test,
    expect(
        clippy::unwrap_used,
        clippy::expect_used,
        reason = "tests may fail fast"
    )
)]
#![deny(unsafe_code)]

mod config;
mod context;
mod error;
mod inner;
mod lane;
mod live;
mod model;
mod observe;
mod router;
mod waterfall;

pub use config::{
    BackupSettings, ClientsSettings, CoreSettings, DelegationSettings, HarnessSettings,
    MindSettings, ServerSettings,
};
pub use context::{ContextRegistry, format_recovery_note};
pub use error::{CancelQueued, KernelError, RunOutcome};
pub use inner::{derive_thought_from_thinking, model_visible_for, split_surface_and_inner};
pub use lane::{LaneHandle, LaneOptions};
pub use live::{LiveBus, LiveEvent, LiveSubscription};
pub use model::{ConversationModel, EchoModel, ModelGeneration, ModelRequest, ToolCall};
pub use observe::{ObserveHandle, Span, SpanGuard, SpanRing, spans_leak_content};
pub use router::{SurfaceRouter, SurfaceToolOutcome};
pub use waterfall::{EmitBus, HookEvent, LoopHooks, Waterfall, WaterfallNext};

pub use ene_session::{
    DisplayDepth, EventKind, EventPayload, ProjectOptions, ProjectedHistory, RecoveryReport,
    SessionId, SessionStore, SoulId, TurnId, derive_messages, hash_model_visible, hash_projected,
};

#[cfg(test)]
mod tests;
