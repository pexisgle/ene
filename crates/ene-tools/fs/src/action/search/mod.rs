mod search_glob;
mod search_grep;

use crate::sandbox::SandboxConfig;
use ene_tool_proto::ToolError;

pub const MAX_RESULTS: usize = 100;

pub async fn glob_search(
    pattern: &str,
    path: Option<&str>,
    sandbox: &SandboxConfig,
) -> Result<String, ToolError> {
    search_glob::glob_search(pattern, path, sandbox).await
}

pub async fn grep_search(
    pattern: &str,
    path: Option<&str>,
    include: Option<&str>,
    sandbox: &SandboxConfig,
) -> Result<String, ToolError> {
    search_grep::grep_search(pattern, path, include, sandbox).await
}
