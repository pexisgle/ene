//! Cross-subsystem event bus.
//!
//! Producers (core session, tray, hotkey handlers) push into a single
//! `UnboundedSender`. The winit event loop owns the receiver and
//! drains it on the main thread.
#![expect(
    dead_code,
    reason = "event variants stay for inner/detail consumers that core has not started emitting yet"
)]
use tokio::sync::mpsc;

use crate::settings::UserInputPrompt;

pub type AppEventSender = mpsc::UnboundedSender<AppEvent>;
pub type AppEventReceiver = mpsc::UnboundedReceiver<AppEvent>;

#[derive(Debug, Clone)]
pub enum AppEvent {
    Tray(TrayAction),
    Ai(AiStreamUpdate),
    PerformanceCue(String),
    MotionCue {
        name: String,
        layer: ene_card::MotionLayer,
        priority: u8,
        duration: f32,
    },
    ExpressionCue {
        name: String,
        weight: f32,
        hold_secs: f64,
        target_time: f64,
    },
    CancelCue {
        scope: String,
    },
    LookAtCue {
        target: String,
    },
    BeatPulse {
        bpm: f32,
        intensity: f32,
    },
    #[cfg(feature = "voice")]
    MicStateChanged {
        active: bool,
    },
    Quit,
    RuntimeDisconnected,
    PendingCandidatesCount(usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayAction {
    OpenSettings {
        page: Option<crate::settings_ui::PageKind>,
    },
    OpenChat,
    OpenDetail,
    Quit,
}

#[derive(Debug, Clone)]
pub enum AiStreamUpdate {
    TextDelta(String),
    ToolCallStart {
        name: String,
        arguments: String,
    },
    ToolCallResult {
        name: String,
        result: String,
    },
    PermissionRequired {
        request_id: String,
        action: String,
        target: String,
        description: String,
    },
    UserInputRequired {
        request_id: String,
        prompt: UserInputPrompt,
    },
    Finished,
    Error(String),
}
