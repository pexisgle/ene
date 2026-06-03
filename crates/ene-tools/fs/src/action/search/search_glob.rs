use super::MAX_RESULTS;
use crate::utils::sandbox::SandboxConfig;
use ene_tool_proto::ToolError;
use std::path::Path;

pub async fn glob_search(
    pattern: &str,
    path: Option<&str>,
    sandbox: &SandboxConfig,
) -> Result<String, ToolError> {
    let base = if let Some(p) = path {
        let resolved = sandbox.resolve_and_check(Path::new(p), false)?;
        resolved
    } else {
        std::env::current_dir().unwrap_or_else(|_| Path::new(".").to_path_buf())
    };

    if !base.is_dir() {
        return Err(ToolError::ExecutionFailed {
            message: format!("glob path must be a directory: {}", base.display()),
        });
    }

    let pattern_path = base.join(pattern);
    let glob_pattern = pattern_path.to_string_lossy().to_string();

    let mut files = Vec::new();
    match glob::glob(&glob_pattern) {
        Ok(entries) => {
            for entry in entries.flatten() {
                files.push(entry.to_string_lossy().to_string());
                if files.len() > MAX_RESULTS {
                    break;
                }
            }
        }
        Err(e) => {
            return Err(ToolError::ExecutionFailed {
                message: format!("Invalid glob pattern: {e}"),
            });
        }
    }

    let truncated = files.len() > MAX_RESULTS;
    if truncated {
        files.truncate(MAX_RESULTS);
    }

    files.sort();

    let mut output = Vec::new();
    if files.is_empty() {
        output.push("No files found".to_string());
    } else {
        output.extend(files);
        if truncated {
            output.push("".to_string());
            output.push(format!(
                "(Results are truncated: showing first {} results. Consider using a more specific path or pattern.)",
                MAX_RESULTS
            ));
        }
    }

    Ok(output.join("\n"))
}

use ene_tool_proto::{
    KeywordSet, SideEffects, ToolCategory, ToolExample, ToolName, ToolSpec, ToolVersion,
};
use ene_tools_common::ToolAction;
use std::sync::{Arc, RwLock};

/// Filesystem sub-action to execute glob search.
pub struct FsGlobSubAction {
    sandbox: Arc<RwLock<Option<Arc<crate::utils::sandbox::Sandbox>>>>,
}

impl FsGlobSubAction {
    /// Creates a new `FsGlobSubAction` with the shared sandbox reference.
    pub fn new(sandbox: Arc<RwLock<Option<Arc<crate::utils::sandbox::Sandbox>>>>) -> Self {
        Self { sandbox }
    }
}

#[async_trait::async_trait]
impl ToolAction for FsGlobSubAction {
    fn tool_name(&self) -> &'static str {
        "filesystem.glob"
    }

    fn definition(&self) -> ToolSpec {
        ToolSpec {
            name: ToolName::new("filesystem.glob"),
            version: ToolVersion::default(),
            display_name: "Glob Search".to_string(),
            summary: "Find files and directories matching a glob pattern.".to_string(),
            description: "Find files and directories matching a glob pattern.".to_string(),
            category: ToolCategory::Filesystem,
            keywords: KeywordSet::primary_only([
                "glob", "find", "search", "files", "pattern", "match",
            ]),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "pattern": {
                        "type": "string",
                        "description": "Glob pattern (e.g. '**/*.rs', 'src/**/*.ts')"
                    },
                    "path": {
                        "type": "string",
                        "description": "Base directory to search from (defaults to cwd)"
                    }
                },
                "required": ["pattern"]
            }),
            examples: vec![
                ToolExample {
                    description: "Find all Rust source files".to_string(),
                    input: serde_json::json!({"pattern": "**/*.rs", "path": "/home/user/project"}),
                    output: Some(
                        "/home/user/project/src/main.rs\n/home/user/project/src/lib.rs".to_string(),
                    ),
                },
                ToolExample {
                    description: "Find all JSON config files".to_string(),
                    input: serde_json::json!({"pattern": "config/**/*.json"}),
                    output: None,
                },
            ],
            caveats: Vec::new(),
            side_effects: SideEffects::default(),
            preconditions: Vec::new(),
            related: Vec::new(),
        }
    }

    async fn execute(&self, arguments: &str) -> Result<String, ToolError> {
        let sandbox = {
            let guard = self.sandbox.read().unwrap_or_else(|e| e.into_inner());
            guard.clone().unwrap_or_else(|| {
                Arc::new(crate::utils::sandbox::Sandbox::new(Default::default()))
            })
        };

        let args: serde_json::Value =
            serde_json::from_str(arguments).map_err(|e| ToolError::InvalidArguments {
                message: format!("Failed to parse glob arguments: {e}"),
            })?;

        let pattern = args["pattern"]
            .as_str()
            .ok_or_else(|| ToolError::InvalidArguments {
                message: "pattern is required for glob".to_string(),
            })?;
        let path = args["path"].as_str();
        glob_search(pattern, path, sandbox.config()).await
    }
}
