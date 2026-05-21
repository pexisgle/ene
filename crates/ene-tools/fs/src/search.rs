mod glob;
mod grep;

use super::definition::ToolDefinition;
use crate::error::ToolError;
use crate::sandbox::SandboxConfig;

const MAX_RESULTS: usize = 100;

    pub fn glob_tool_definition() -> ToolDefinition {
    ToolDefinition {
        name: "glob".to_string(),
        description: "Fast file pattern matching tool that works with any codebase size. Supports glob patterns like '**/*.rs' or 'src/**/*.ts'. Returns matching file paths sorted by modification time.".to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "pattern": { "type": "string", "description": "The glob pattern to match files against" },
                "path": { "type": "string", "description": "The directory to search in. Defaults to current working directory." }
            },
            "required": ["pattern"]
        }),
        category: Some(super::ToolCategory::Filesystem),
        keywords: vec!["glob".to_string(), "search".to_string(), "find".to_string(), "files".to_string()],
    }
}

pub fn grep_tool_definition() -> ToolDefinition {
    ToolDefinition {
        name: "grep".to_string(),
        description: "Fast content search tool that works with any codebase size. Searches file contents using regular expressions. Supports full regex syntax. Filter files by pattern with the include parameter.".to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "pattern": { "type": "string", "description": "The regex pattern to search for in file contents" },
                "path": { "type": "string", "description": "The directory to search in. Defaults to current working directory." },
                "include": { "type": "string", "description": "File pattern to include in the search (e.g. '*.rs', '*.{ts,tsx}')" }
            },
            "required": ["pattern"]
        }),
        category: Some(super::ToolCategory::Filesystem),
        keywords: vec!["grep".to_string(), "search".to_string(), "regex".to_string(), "content".to_string()],
    }
}

pub async fn glob_search(
    pattern: &str,
    path: Option<&str>,
    sandbox: &SandboxConfig,
) -> Result<String, ToolError> {
    glob::glob_search(pattern, path, sandbox).await
}

pub async fn grep_search(
    pattern: &str,
    path: Option<&str>,
    include: Option<&str>,
    sandbox: &SandboxConfig,
) -> Result<String, ToolError> {
    grep::grep_search(pattern, path, include, sandbox).await
}
