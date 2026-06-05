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
            if action.name() == name {
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
