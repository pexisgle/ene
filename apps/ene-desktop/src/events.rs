//! Cross-subsystem event bus.
//!
//! Producers (AI bridge, tray, hotkey handlers) push into a single
//! `UnboundedSender`. The winit event loop owns the receiver and
//! drains it on the main thread.
use ene_plugin_proto::UserInputPrompt;
use ene_runtime::RequestId;
use tokio::sync::mpsc;

/// Cheap to clone (`Sender` is `Send + Sync`).
pub type AppEventSender = mpsc::UnboundedSender<AppEvent>;

/// Owned by the [`Runtime`](crate::runtime::Runtime); consumed by
/// `try_recv` loops.
pub type AppEventReceiver = mpsc::UnboundedReceiver<AppEvent>;

/// Top-level event enum. Variants are split by producer for clarity
/// but share the same bus.
#[derive(Debug, Clone)]
pub enum AppEvent {
    /// System tray (or future global hotkey) actions.
    Tray(TrayAction),
    /// Streamed AI events mirrored from [`ene_runtime::EneEvent`].
    Ai(AiStreamUpdate),
    /// Raw performance cue name from [`ene_runtime::EneEvent::Performance`].
    /// Desktop maps this to VRM playback; do not forward `SpecialToken` /
    /// Expression events (removed in API v1).
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "PerformanceCue retained for motion playback wiring"
        )
    )]
    PerformanceCue(String),
    /// Motion cue with layer routing information (#133).
    ///
    /// `layer` carries the canonical [`ene_config::MotionLayer`] from the
    /// performance cue; the consumer converts it to the rendering-side
    /// `ene_vrm::MotionLayer` at the boundary.
    MotionCue {
        name: String,
        layer: ene_config::MotionLayer,
        priority: u8,
        duration: f32,
    },
    /// Expression cue with weight and hold duration (#132).
    ExpressionCue {
        name: String,
        weight: f32,
        hold_secs: f64,
    },
    /// Cancel cue (#132).
    CancelCue { scope: String },
    /// `LookAt` cue — gaze target hint from LLM performance markers.
    /// Forwarded to VRM gaze when the gaze system is implemented.
    LookAtCue { target: String },
    /// Microphone capture started (`active = true`) or stopped
    /// (`active = false`). Emitted by the audio capture subsystem so
    /// the chat UI can refresh its mic-toggle indicator.
    #[cfg(feature = "voice")]
    MicStateChanged { active: bool },
    /// Request the event loop to exit.
    Quit,
    /// The runtime actor broadcast channel closed (#242).
    RuntimeDisconnected,
    /// Pending memory candidates available for user approval (#223).
    PendingCandidatesCount(usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayAction {
    /// Open the settings window. `page = None` keeps the
    /// previously-focused page; `page = Some(_)` jumps the tab strip
    /// to that page (used to focus the AI page on a permission /
    /// user-input prompt).
    OpenSettings {
        page: Option<crate::settings_ui::PageKind>,
    },
    OpenChat,
    Quit,
}

/// Flattened subset of [`ene_runtime::EneEvent`] the UI layer cares
/// about. Pipeline diagnostics stay off this bus (use `diagnostics()`).
#[derive(Debug, Clone)]
pub enum AiStreamUpdate {
    TextDelta(String),
    ToolCallStart {
        #[expect(dead_code, reason = "yet to be wired to tool call UI rendering")]
        name: String,
        #[expect(dead_code, reason = "yet to be wired to tool call UI rendering")]
        arguments: String,
    },
    ToolCallResult {
        #[expect(dead_code, reason = "yet to be wired to tool call UI rendering")]
        name: String,
        #[expect(dead_code, reason = "yet to be wired to tool call UI rendering")]
        result: String,
    },
    PermissionRequired {
        request_id: RequestId,
        action: String,
        target: String,
        description: String,
    },
    UserInputRequired {
        request_id: RequestId,
        prompt: UserInputPrompt,
    },
    Finished,
    Error(String),
}
