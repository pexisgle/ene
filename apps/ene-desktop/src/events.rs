//! Cross-subsystem event bus.
//!
//! Producers (AI bridge, tray, hotkey handlers) push into a single
//! `UnboundedSender`. The winit event loop owns the receiver and
//! drains it on the main thread.
use ene_core::RequestId;
use ene_tool_proto::UserInputPrompt;
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
    /// Streamed AI events mirrored from [`ene_core::EneEvent`].
    Ai(AiStreamUpdate),
    /// Raw `<|emo:NAME|>` token extracted by the AI bridge before
    /// forwarding. Currently logged only.
    EmoteToken(String),
    /// Request the event loop to exit.
    Quit,
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

/// Flattened subset of [`ene_core::EneEvent`] the UI layer cares
/// about. `StatusChanged` / `SessionSplit` are dropped by the bridge.
#[derive(Debug, Clone)]
pub enum AiStreamUpdate {
    TextDelta(String),
    ToolCallStart {
        #[expect(dead_code)]
        name: String,
        #[expect(dead_code)]
        arguments: String,
    },
    ToolCallResult {
        #[expect(dead_code)]
        name: String,
        #[expect(dead_code)]
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
    TaskProgress {
        #[expect(dead_code)]
        task_id: String,
        #[expect(dead_code)]
        step: usize,
        #[expect(dead_code)]
        total_steps: Option<usize>,
        #[expect(dead_code)]
        description: String,
    },
    Finished,
    #[expect(dead_code)]
    Error(String),
}
