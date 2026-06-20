//! Cross-subsystem event bus.
//!
//! All `Producer`s (AI bridge tokio task, system-tray thread/GTK pump)
//! push into a single [`tokio::sync::mpsc::UnboundedSender`]. The winit
//! event loop owns the [`UnboundedReceiver`] and drains it in
//! `ApplicationHandler::about_to_wait` to keep every mutation on the
//! main thread.
use ene_core::RequestId;
use ene_tool_proto::UserInputPrompt;
use tokio::sync::mpsc;

/// Single source of truth for cross-subsystem events. Cheap to clone
/// (`Sender` is `Send + Sync`).
pub type AppEventSender = mpsc::UnboundedSender<AppEvent>;

/// Owned by the [`Runtime`](crate::runtime::Runtime); consumed by
/// `try_recv` loops.
pub type AppEventReceiver = mpsc::UnboundedReceiver<AppEvent>;

/// Top-level event enum. Variants are split by producer for clarity,
/// but all share the same bus.
#[allow(dead_code)] // PR1 infrastructure; PR2+ consumes every variant.
#[derive(Debug, Clone)]
pub enum AppEvent {
    /// System tray (or future global hotkey) actions.
    Tray(TrayAction),
    /// Streamed AI events mirrored from [`ene_core::EneEvent`].
    Ai(AiStreamUpdate),
    /// Raw `<|emo:NAME|>` token extracted by the AI bridge before
    /// forwarding. (PR2 will route it to the emotion queue; today it
    /// is logged only.)
    EmoteToken(String),
    /// Request the event loop to exit. The runtime forwards this to
    /// `event_loop.exit()`.
    Quit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayAction {
    /// Open the settings window. `page = None` keeps the
    /// previously-focused page (defaults to Character on first
    /// show); `page = Some(_)` jumps the tab strip to that page
    /// — used by the runtime to focus the AI page when a
    /// `PermissionRequired` or `UserInputRequired` event arrives.
    OpenSettings {
        page: Option<crate::settings_ui::PageKind>,
    },
    Quit,
}

/// Flattened subset of [`ene_core::EneEvent`] the UI layer cares
/// about. The actor's `StatusChanged` / `SessionSplit` events are
/// dropped by the bridge before reaching here.
#[allow(dead_code)] // PR1 enqueues; PR2 UI consumes each variant's fields.
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
        task_id: String,
        step: usize,
        total_steps: Option<usize>,
        description: String,
    },
    Finished,
    Error(String),
}
