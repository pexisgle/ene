use crate::action;
use async_trait::async_trait;
use ene_tool_common::ToolAction;
use ene_tool_proto::{ToolError, ToolProvider, ToolSpec};

/// App tool provider managing GUI automation tasks.
pub struct AppToolProvider {
    actions: Vec<Box<dyn ToolAction>>,
}

impl AppToolProvider {
    /// Creates a new `AppToolProvider` and registers all 15 individual tool actions.
    pub fn new() -> Self {
        let actions: Vec<Box<dyn ToolAction>> = vec![
            // Window management (5)
            Box::new(action::ListWindowsAction),
            Box::new(action::FocusWindowAction),
            Box::new(action::GetActiveWindowAction),
            Box::new(action::ListMonitorsAction),
            Box::new(action::CaptureWindowAction),
            // Input simulation (5)
            Box::new(action::TypeTextAction),
            Box::new(action::PressKeyAction),
            Box::new(action::KeyComboAction),
            Box::new(action::MouseMoveAction),
            Box::new(action::MouseClickAction),
            Box::new(action::MouseDragAction),
            Box::new(action::MouseScrollAction),
            // Screen capture (1)
            Box::new(action::ScreenshotAction),
            // Clipboard (2)
            Box::new(action::ClipboardReadAction),
            Box::new(action::ClipboardWriteAction),
        ];
        Self { actions }
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
        self.actions.iter().map(|a| a.definition()).collect()
    }
    async fn call_tool(&self, name: &str, arguments: &str) -> Result<String, ToolError> {
        for action in &self.actions {
            if action.tool_name() == name {
                return action.execute(arguments).await;
            }
        }
        Err(ToolError::NotFound {
            tool_name: name.to_string(),
        })
    }

    fn set_session_id(&self, _session_id: &str) {
        // App tools are stateless
    }
}
