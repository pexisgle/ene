use async_trait::async_trait;
use ene_tool_proto::{ToolDefinition, ToolError, ToolProvider};
use std::sync::{Arc, RwLock};

use crate::sandbox::Sandbox;

pub struct FsToolProvider {
    sandbox: Arc<RwLock<Option<Arc<Sandbox>>>>,
}

impl FsToolProvider {
    pub fn new() -> Self {
        Self {
            sandbox: Arc::new(RwLock::new(None)),
        }
    }
}

impl Default for FsToolProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ToolProvider for FsToolProvider {
    fn list_tools(&self) -> Vec<ToolDefinition> {
        vec![
            crate::filesystem::tool_definition(),
            crate::action::shell::tool_definition(),
            crate::action::undo::tool_definition(),
        ]
    }

    async fn call_tool(&self, name: &str, arguments: &str) -> Result<String, ToolError> {
        let sandbox = {
            let guard = self.sandbox.read().unwrap_or_else(|e| e.into_inner());
            guard
                .clone()
                .unwrap_or_else(|| Arc::new(Sandbox::new(Default::default())))
        };
        let session_id = sandbox.session_id();

        match name {
            "filesystem" => {
                let args: serde_json::Value =
                    serde_json::from_str(arguments).map_err(|e| ToolError::InvalidArguments {
                        message: format!("Failed to parse filesystem arguments: {e}"),
                    })?;
                let action =
                    args["action"]
                        .as_str()
                        .ok_or_else(|| ToolError::InvalidArguments {
                            message: "Missing 'action' field in filesystem arguments".to_string(),
                        })?;
                crate::filesystem::execute(
                    action,
                    &args,
                    &sandbox,
                    sandbox.undo_manager(),
                    &session_id,
                )
                .await
            }
            "shell" => {
                let args: serde_json::Value =
                    serde_json::from_str(arguments).map_err(|e| ToolError::InvalidArguments {
                        message: format!("Failed to parse shell arguments: {e}"),
                    })?;
                let command =
                    args["command"]
                        .as_str()
                        .ok_or_else(|| ToolError::InvalidArguments {
                            message: "Missing 'command' field".to_string(),
                        })?;
                let description = args["description"].as_str().unwrap_or("");
                let timeout = args["timeout"].as_u64();
                let workdir = args["workdir"].as_str();

                sandbox.check_permission(
                    crate::permission::DestructiveAction::ShellCommand,
                    command,
                    description,
                )?;

                crate::action::shell::shell_exec(
                    command,
                    description,
                    timeout,
                    workdir,
                    sandbox.config(),
                )
                .await
            }
            "undo" => crate::action::undo::undo(sandbox.undo_manager(), &session_id).await,
            _ => Err(ToolError::NotFound {
                tool_name: name.to_string(),
            }),
        }
    }

    fn set_session_id(&self, session_id: &str) {
        let guard = self.sandbox.read().unwrap_or_else(|e| e.into_inner());
        if let Some(s) = guard.as_ref() {
            s.set_session_id(session_id);
        }
    }

    fn set_sandbox(&self, data: &ene_tool_proto::SandboxConfigData) {
        let new_sandbox = Arc::new(Sandbox::new(data.clone().into()));
        let mut guard = self.sandbox.write().unwrap_or_else(|e| e.into_inner());
        *guard = Some(new_sandbox);
    }

    fn approve_permission(&self, request_id: &str) {
        let guard = self.sandbox.read().unwrap_or_else(|e| e.into_inner());
        if let Some(s) = guard.as_ref() {
            s.approve_request(request_id);
        }
    }

    fn allow_pattern(&self, action: &str, target_pattern: &str) {
        let guard = self.sandbox.read().unwrap_or_else(|e| e.into_inner());
        if let Some(s) = guard.as_ref() {
            s.allow_pattern(action, target_pattern);
        }
    }
}
