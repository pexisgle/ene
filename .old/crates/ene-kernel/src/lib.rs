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

mod compact;
mod config;
mod context;
mod context_window;
mod error;
mod inner;
mod lane;
mod live;
mod model;
mod observe;
mod retry;
mod router;
mod speech;
mod tool_result;
mod waterfall;

pub use config::{
    AiSettings, AiTasks, BackupSettings, ClientsSettings, ContextSettings, CoreSettings,
    DelegationSettings, HarnessSettings, LaneMindSettings, PluginIpcSettings, PluginPolicySettings,
    PluginProfileKind, PluginSettings, RetrySettings, ServerSettings, TaskBinding, TokenEstimation,
    ToolOutputSettings,
};
pub use context::{ContextRegistry, SOURCE_ORDER, canonicalize_source_key, format_recovery_note};
pub use context_window::{
    DEFAULT_CONTEXT_WINDOW, EffectiveWindow, effective_window, estimate_tokens, fit_prompt,
    fit_prompt_llm,
};
pub use error::{CancelQueued, KernelError, RunOutcome};
pub use inner::{derive_thought_from_thinking, model_visible_for, split_surface_and_inner};
pub use lane::{LaneHandle, LaneOptions};
pub use live::{LiveBus, LiveEvent, LiveSubscription};
pub use model::{
    ConversationModel, EchoModel, ModelGeneration, ModelRequest, TextDeltaSink, ToolCall,
    ToolCallingModel,
};
pub use observe::{ObserveHandle, Span, SpanGuard, SpanRing, spans_leak_content};
pub use retry::{is_retryable_provider_failure, retry_call};
pub use router::{SurfaceRouter, SurfaceToolOutcome};
pub use speech::{SpeechPresenter, TurnFinalizer, TurnPrefetch};
pub use waterfall::{EmitBus, HookEvent, LoopHooks, Waterfall, WaterfallGuard, WaterfallNext};

pub use ene_session::{
    DisplayDepth, EventKind, EventPayload, ProjectOptions, ProjectedHistory, RecoveryReport,
    SessionId, SessionStore, SoulId, TurnId, derive_messages, hash_model_visible, hash_projected,
};

#[cfg(test)]
mod tests {
    use crate::LaneMindSettings as MindSettings;
    include!("tests.rs");
}
