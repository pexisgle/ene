use super::MAX_RESULTS;
use crate::utils::sandbox::SandboxConfig;
use crate::utils::{SandboxRef, default_sandbox, resolve_sandbox};
use ene_plugin::prelude::*;
use std::path::Path;

pub fn glob_search(
    pattern: &str,
    path: Option<&str>,
    sandbox: &SandboxConfig,
) -> Result<String, ToolError> {
    let base = if let Some(p) = path {
        sandbox.resolve_and_check(Path::new(p), false)?
    } else {
        std::env::current_dir().unwrap_or_else(|_| Path::new(".").to_path_buf())
    };

    if !base.is_dir() {
        return Err(ToolError::execution_failed(format!(
            "glob path must be a directory: {}",
            base.display()
        )));
    }

    let pattern_path = base.join(pattern);
    let glob_pattern = pattern_path.to_string_lossy().to_string();

    let mut files = Vec::new();
    match glob::glob(&glob_pattern) {
        Ok(entries) => {
            for entry in entries.flatten() {
                files.push(entry.to_string_lossy().to_string());
                if files.len() >= MAX_RESULTS {
                    break;
                }
            }
        }
        Err(e) => {
            return Err(ToolError::execution_failed(format!(
                "Invalid glob pattern: {e}"
            )));
        }
    }

    files.sort();

    let truncated = files.len() > MAX_RESULTS;
    if truncated {
        files.truncate(MAX_RESULTS);
    }

    let mut output = Vec::new();
    if files.is_empty() {
        output.push("No files found".to_string());
    } else {
        output.extend(files);
        if truncated {
            output.push(String::new());
            output.push(format!(
                "(Results are truncated: showing first {MAX_RESULTS} results. Consider using a more specific path or pattern.)"
            ));
        }
    }

    Ok(output.join("\n"))
}

#[derive(Clone, Deserialize, JsonSchema, ToolAction)]
#[tool(
    namespace = "filesystem",
    name = "glob",
    summary = "Find files and directories matching a glob pattern.",
    description = "Find files and directories matching a glob pattern.",
    category = "Filesystem",
    keywords_primary = "glob, find, search, files, pattern, match",
    side_effects = "ReadOnly"
)]
pub struct FsGlobAction {
    /// Glob pattern (e.g. '**/*.rs', 'src/**/*.ts').
    pattern: String,
    /// Base directory to search from (defaults to cwd).
    #[serde(default)]
    path: Option<String>,

    #[tool(skip)]
    #[serde(skip, default = "default_sandbox")]
    sandbox: SandboxRef,
}

impl FsGlobAction {
    pub const fn new(sandbox: SandboxRef) -> Self {
        Self {
            pattern: String::new(),
            path: None,
            sandbox,
        }
    }

    async fn run(&self) -> Result<String, ToolError> {
        let sandbox = resolve_sandbox(&self.sandbox);

        glob_search(&self.pattern, self.path.as_deref(), sandbox.config())
    }
}
