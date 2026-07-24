use crate::action;
use async_trait::async_trait;
use ene_plugin_proto::{ToolError, ToolProvider, ToolSpec};
use ene_tool_common::{ActionSetProvider, ToolAction};

/// App tool provider managing GUI automation tasks.
///
/// Dispatch is handled by [`ActionSetProvider`] (#139).
pub struct AppToolProvider {
    inner: ActionSetProvider,
}

impl AppToolProvider {
    /// Creates a new `AppToolProvider` and registers all app tool actions.
    pub fn new() -> Self {
        let actions: Vec<Box<dyn ToolAction>> = vec![
            // Window management (5)
            Box::new(action::ListWindowsAction::default()),
            Box::new(action::FocusWindowAction::default()),
            Box::new(action::GetActiveWindowAction::default()),
            Box::new(action::ListMonitorsAction::default()),
            Box::new(action::CaptureWindowAction::default()),
            // Input simulation (5)
            Box::new(action::TypeTextAction::default()),
            Box::new(action::PressKeyAction::default()),
            Box::new(action::KeyComboAction::default()),
            Box::new(action::MouseMoveAction::default()),
            Box::new(action::MouseClickAction::default()),
            Box::new(action::MouseDragAction::default()),
            Box::new(action::MouseScrollAction::default()),
            // Screen capture (1)
            Box::new(action::ScreenshotAction::default()),
            // Clipboard (2)
            Box::new(action::ClipboardReadAction::default()),
            Box::new(action::ClipboardWriteAction::default()),
        ];
        Self {
            inner: ActionSetProvider::new(actions),
        }
    }
}

impl Default for AppToolProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ToolProvider for AppToolProvider {
    fn list_specs(&self) -> Vec<ToolSpec> {
        self.inner.list_specs()
    }

    async fn call_tool(&self, name: &str, arguments: &str) -> Result<String, ToolError> {
        self.inner.call_tool(name, arguments).await
    }

    fn set_call_context(&self, ctx: &ene_plugin_proto::CallContext) {
        self.inner.set_call_context(ctx);
    }
}
