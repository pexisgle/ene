use crate::action;
use crate::config::app_tool_definition;
use async_trait::async_trait;
use ene_tool_proto::{ToolDefinition, ToolError, ToolProvider};
use ene_tools_common::ToolAction;
use std::collections::HashMap;

/// Dynamic ToolAction implementation for the app tool.
pub struct AppAction {
    sub_actions: HashMap<String, Box<dyn ToolAction>>,
}

impl AppAction {
    /// Creates a new `AppAction` and registers all sub-actions.
    pub fn new() -> Self {
        let actions: Vec<Box<dyn ToolAction>> = vec![
            Box::new(action::ClipboardReadAction),
            Box::new(action::ClipboardWriteAction),
            Box::new(action::ListWindowsAction),
            Box::new(action::FocusWindowAction),
            Box::new(action::GetActiveWindowAction),
            Box::new(action::ListMonitorsAction),
            Box::new(action::TypeTextAction),
            Box::new(action::PressKeyAction),
            Box::new(action::KeyComboAction),
            Box::new(action::MouseMoveAction),
            Box::new(action::MouseClickAction),
            Box::new(action::MouseDragAction),
            Box::new(action::MouseScrollAction),
            Box::new(action::ScreenshotAction),
            Box::new(action::CaptureWindowAction),
        ];

        let mut sub_actions = HashMap::new();
        for act in actions {
            sub_actions.insert(act.definition().name.clone(), act);
        }

        Self { sub_actions }
    }
}

#[async_trait]
impl ToolAction for AppAction {
    fn definition(&self) -> ToolDefinition {
        app_tool_definition()
    }

    async fn execute(&self, arguments: &str) -> Result<String, ToolError> {
        let args: serde_json::Value =
            serde_json::from_str(arguments).map_err(|e| ToolError::InvalidArguments {
                message: format!("Invalid arguments for app: {e}"),
            })?;
        let sub_action_name =
            args["action"]
                .as_str()
                .ok_or_else(|| ToolError::InvalidArguments {
                    message: "Missing 'action' field in app arguments".to_string(),
                })?;

        if let Some(sub_act) = self.sub_actions.get(sub_action_name) {
            sub_act.execute(arguments).await
        } else {
            Err(ToolError::InvalidArguments {
                message: format!("Unknown app action: {sub_action_name}"),
            })
        }
    }
}

/// App tool provider managing GUI automation tasks.
pub struct AppToolProvider {
    actions: Vec<Box<dyn ToolAction>>,
}

impl AppToolProvider {
    /// Creates a new `AppToolProvider` and registers the app action.
    pub fn new() -> Self {
        Self {
            actions: vec![Box::new(AppAction::new())],
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
    fn list_tools(&self) -> Vec<ToolDefinition> {
        self.actions.iter().map(|a| a.definition()).collect()
    }

    async fn call_tool(&self, name: &str, arguments: &str) -> Result<String, ToolError> {
        for action in &self.actions {
            if action.definition().name == name {
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
